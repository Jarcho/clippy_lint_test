use anyhow::{bail, Context, Result};
use cargo_metadata::{diagnostic::DiagnosticLevel, CompilerMessage, Message};
use clippy_lint_test::{is_rustc_crate, CrateId, LatestVersions};
use flate2::read::GzDecoder;
use regex::{Regex, RegexBuilder};
use rm_rf::remove;
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    str,
};
use tar::Archive;

#[derive(argh::FromArgs)]
/// Tests clippy lints on all downloaded crates
struct Args {
    /// clippy directory
    #[argh(positional)]
    clippy_dir: PathBuf,

    /// the name of the report file (default `CLIPPY_BRANCH_NAME-CURRENT_DATE.txt`)
    #[argh(option, short = 'r', long = "report-file")]
    report_name: Option<PathBuf>,

    /// lints to test
    #[argh(option, short = 'l', long = "lint")]
    lints: Vec<String>,

    /// regex filter of which messages to accept
    #[argh(option, short = 'f', long = "filter")]
    filter: Option<String>,

    /// the number of crates to compile before clearing the target directory (default 500)
    #[argh(option, long = "cache-size")]
    cache_size: Option<usize>,
}

fn main() -> Result<()> {
    let args: Args = argh::from_env();
    let filter = args
        .filter
        .map(|f| {
            RegexBuilder::new(&f)
                .build()
                .with_context(|| format!("error parsing `{}`", f))
        })
        .transpose()?;
    let cache_size = args.cache_size.unwrap_or(500);

    println!("Compiling clippy...");
    let clippy_args = compile_clippy(&args.clippy_dir)?;

    let mut report = io::BufWriter::new(
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(args.report_name.unwrap_or_else(|| {
                let res = Command::new("git")
                    .args(["branch", "--show-current"])
                    .current_dir(&args.clippy_dir)
                    .output();
                let name = res.map_or(None, |res| {
                    res.status
                        .success()
                        .then(|| ())
                        .and_then(|()| String::from_utf8(res.stdout).ok())
                });
                let date = chrono::Local::today().format("%Y-%m-%d");
                if let Some(name) = name {
                    format!("{}-{}.txt", name, date)
                } else {
                    format!("{}.txt", date)
                }
                .into()
            }))
            .context("error creating report file")?,
    );

    let mut lint_counters = args
        .lints
        .into_iter()
        .map(|name| {
            let name = name.replace('-', "_");
            let name = if !name.starts_with("clippy::") {
                format!("clippy::{}", name)
            } else {
                name
            };
            (name, 0usize)
        })
        .collect::<HashMap<_, _>>();
    let mut per_crate_count = HashMap::<&str, (usize, bool)>::new();

    let home_dir = home::cargo_home().context("error finding cargo home dir")?;
    let crates_dir = home_dir
        .join("registry")
        .join("cache")
        .join("github.com-1ecc6299db9ec823");
    let crates = find_crates(&crates_dir)?;
    let mut crate_ids = Vec::with_capacity(crates.len() * 2);
    for (name, versions) in crates {
        crate_ids.extend(versions.iter_ids(&name).map(|x| x.to_string()));
    }
    let crates = crate_ids;

    let temp_dir = temp_dir::TempDir::new().expect("error creating temp dir");
    let temp_dir = temp_dir.path();
    let target_dir = temp_dir.join("target");

    for (i, krate) in crates.iter().enumerate() {
        if i > 0 && i % cache_size == 0 {
            // Don't let the target directory get too big.
            let _ = remove(&target_dir);
        }

        println!("Checking crate `{}`...", krate);
        print!("{}/{}\r", i + 1, crates.len());
        let _ = io::stdout().flush();
        match check_crate(
            &clippy_args,
            &target_dir,
            &mut lint_counters,
            &crates_dir,
            krate,
            filter.as_ref(),
            temp_dir,
        ) {
            Ok(output) => {
                if !output.lint_msgs.is_empty() {
                    println!("Found {} warnings", output.lint_msgs.len());
                    write!(report, "{}: {} warnings\n\n", krate, output.lint_msgs.len())
                        .context("error writing report")?;
                    for m in &output.lint_msgs {
                        report
                            .write_all(m.as_bytes())
                            .context("error writing report")?;
                    }
                    writeln!(report).context("error writing report")?;
                    report.flush().context("error writing report")?;
                    per_crate_count.entry(krate).or_default().0 = output.lint_msgs.len();
                }
                if !output.ice_msg.is_empty() {
                    println!();
                    write!(report, "{}: ICE\n\n{}\n", krate, output.ice_msg)
                        .context("error writing report")?;
                    report.flush().context("error writing report")?;
                    per_crate_count.entry(krate).or_default().1 = true;
                }
                if !output.err_msg.is_empty() {
                    println!("{}", output.err_msg);
                }
            }
            Err(e) => eprintln!("{}", e),
        }
    }

    write!(report, "\nReport summary:\n\n").context("error writing report")?;
    for (krate, (count, ice)) in per_crate_count {
        writeln!(
            report,
            "{}: {}{} warning{}",
            krate,
            if ice { "ICE, " } else { "" },
            count,
            if count == 1 { "" } else { "s" },
        )
        .context("error writing report")?
    }
    writeln!(report).context("error writing report")?;
    for (lint, count) in lint_counters {
        writeln!(report, "{}: {} occurrences", lint, count).context("error writing report")?;
    }
    report.flush().context("error writing report")?;

    let _ = remove(&target_dir);
    Ok(())
}

fn find_crates(p: &Path) -> Result<HashMap<String, LatestVersions>> {
    let mut crates = HashMap::<_, LatestVersions>::new();
    for file in fs::read_dir(p).with_context(|| format!("error reading dir `{}`", p.display()))? {
        let file = file.with_context(|| format!("error reading dir `{}`", p.display()))?;
        if let Some(id) = file
            .path()
            .file_stem()
            .and_then(|name| CrateId::parse(name.to_str()?))
        {
            if is_rustc_crate(id.name) {
                // Ignore rustc crates as they likely won't build.
                continue;
            }
            crates.entry(id.name.into()).or_default().push(id.version);
        }
    }
    Ok(crates)
}

fn parse_toml(p: &Path) -> Result<toml::Value> {
    fs::read_to_string(p)
        .with_context(|| format!("error reading `{}`", p.display()))?
        .parse()
        .with_context(|| format!("error parsing file `{}`", p.display()))
}

struct ClippyArgs {
    manifest: OsString,
    channel: String,
}
impl ClippyArgs {
    fn run_command(&self) -> Command {
        let args: [&OsStr; 10] = [
            self.channel.as_ref(),
            "--quiet".as_ref(),
            "run".as_ref(),
            &self.manifest,
            "--release".as_ref(),
            "--cap-lints".as_ref(),
            "allow".as_ref(),
            "--bin".as_ref(),
            "cargo-clippy".as_ref(),
            "--".as_ref(),
        ];
        let mut command = Command::new("cargo");
        command.args(args);
        command
    }
}

fn compile_clippy(p: &Path) -> Result<ClippyArgs> {
    let toolchain = p.join("rust-toolchain");
    let contents = parse_toml(&toolchain)?;
    let mut channel_arg = String::from("+");
    if let toml::Value::Table(contents) = contents {
        if let Some(toml::Value::Table(contents)) = contents.get("toolchain") {
            if let Some(toml::Value::String(channel)) = contents.get("channel") {
                channel_arg.push_str(channel);
            } else {
                bail!(
                    "error parsing `{}`: missing field `channel`",
                    toolchain.display()
                );
            }
        } else {
            bail!(
                "error parsing `{}`: missing table `toolchain`",
                toolchain.display()
            );
        }
    } else {
        bail!(
            "error parsing `{}`: missing root table",
            toolchain.display()
        );
    }
    let mut manifest_arg: OsString = "--manifest-path=".into();
    manifest_arg.push(p.join("Cargo.toml"));

    let args: [&OsStr; 6] = [
        channel_arg.as_ref(),
        "build".as_ref(),
        &manifest_arg,
        "--release".as_ref(),
        "--cap-lints".as_ref(),
        "allow".as_ref(),
    ];
    let output = Command::new("cargo")
        .args(args)
        .output()
        .context("error running `cargo`")?;
    if !output.status.success() {
        bail!(
            "Failed to build clippy ({}):\n{}",
            output.status,
            str::from_utf8(&output.stderr).context("error converting `cargo` output to `str`")?
        );
    }

    Ok(ClippyArgs {
        manifest: manifest_arg,
        channel: channel_arg,
    })
}

struct RemoveOnDrop<'a>(&'a Path);
impl Drop for RemoveOnDrop<'_> {
    fn drop(&mut self) {
        let _ = remove(self.0);
    }
}

#[derive(Default)]
struct RunOutput {
    pub lint_msgs: Vec<String>,
    pub err_msg: String,
    pub ice_msg: String,
}

fn check_crate(
    clippy_args: &ClippyArgs,
    target_dir: &Path,
    lints: &mut HashMap<String, usize>,
    crates_dir: &Path,
    krate: &str,
    filter: Option<&Regex>,
    temp_dir: &Path,
) -> Result<RunOutput> {
    extract_crate(&crates_dir.join(format!("{}.crate", krate)), temp_dir)?;

    let path = temp_dir.join(krate);
    let _delayed = RemoveOnDrop(&path);
    remove_file(&path.join(".cargo").join("config"))?;
    remove_file(&path.join("Cargo.lock"))?;
    let manifest_path = path.join("Cargo.toml");
    prepare_manifest(&manifest_path)?;

    let args: [&OsStr; 14] = [
        "--".as_ref(), // command name
        "--manifest-path".as_ref(),
        manifest_path.as_ref(),
        "--quiet".as_ref(),
        "--message-format=json".as_ref(),
        "--target-dir".as_ref(),
        target_dir.as_ref(),
        "--".as_ref(),
        "--cap-lints".as_ref(),
        "warn".as_ref(),
        "--allow".as_ref(),
        "clippy::all".as_ref(),
        "-C".as_ref(),
        "incremental=false".as_ref(),
    ];
    let mut command = clippy_args.run_command();
    command.args(args);
    for lint in lints.keys() {
        let args: [&OsStr; 2] = ["--warn".as_ref(), lint.as_ref()];
        command.args(args);
    }
    let output = command.output().context("error running `cargo`")?;

    let mut result = RunOutput::default();

    if !output.status.success() {
        result.err_msg = format!("error running clippy ({}):\n", output.status);
    }

    for m in Message::parse_stream(output.stdout.as_slice()) {
        let m = m.context("error parsing `cargo` output")?;
        if let Message::CompilerMessage(CompilerMessage { message: m, .. }) = m {
            match (m.level, m.code, m.rendered) {
                (DiagnosticLevel::Warning, Some(c), Some(m)) => {
                    if let Some(count) = lints.get_mut(&c.code) {
                        if filter.map_or(true, |f| f.is_match(&m)) {
                            *count += 1;
                            result.lint_msgs.push(m);
                        }
                    }
                }
                (DiagnosticLevel::Error, Some(c), Some(m))
                    if (c.code == "E0433" && m.contains("use winapi::"))
                        || (c.code == "E0455" && m.contains("link kind `framework`")) =>
                {
                    // Windows or macos only crate. Don't bother reporting errors.
                    result.err_msg = String::new();
                    return Ok(result);
                }
                (DiagnosticLevel::Error, _, Some(m)) => {
                    result.err_msg.push_str(&m);
                }
                _ => (),
            }
        }
    }

    if !output.status.success() {
        let stderr =
            str::from_utf8(&output.stderr).context("error converting `cargo` stderr to `str`")?;
        if stderr.contains("internal compiler error:") {
            result.ice_msg = stderr.to_owned();
        } else {
            result.err_msg.push_str(stderr);
        }
    }

    Ok(result)
}

fn extract_crate(file: &Path, target: &Path) -> Result<()> {
    let mut archive =
        Archive::new(GzDecoder::new(fs::File::open(file).with_context(|| {
            format!("error opening file `{}`", file.display())
        })?));
    archive
        .unpack(target)
        .with_context(|| format!("error unpacking file `{}`", file.display()))
}

fn prepare_manifest(path: &Path) -> Result<()> {
    let mut contents: toml::Value = fs::read_to_string(path)
        .with_context(|| format!("error reading file `{}`", path.display()))?
        .parse()
        .with_context(|| format!("error parsing file `{}`", path.display()))?;

    if let toml::Value::Table(table) = &mut contents {
        if table.remove("workspace").is_some()
            | table.remove("bench").is_some()
            | table
                .get_mut("dependencies")
                .map_or(false, remove_toml_path_deps)
            | table
                .get_mut("build-dependencies")
                .map_or(false, remove_toml_path_deps)
            | table
                .get_mut("dev-dependencies")
                .map_or(false, remove_toml_path_deps)
            | table
                .iter_mut()
                .filter(|&(name, _)| name.starts_with("target") && name.ends_with("dependencies"))
                .fold(false, |update, (_, value)| {
                    update | remove_toml_path_deps(value)
                })
        {
            fs::write(path, contents.to_string())
                .with_context(|| format!("error writing file `{}`", path.display()))?;
        }
    }

    Ok(())
}

fn remove_file(p: &Path) -> Result<()> {
    match fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        e => e.context(format!("error deleting file `{}`", p.display())),
    }
}

fn remove_toml_path_deps(deps: &mut toml::Value) -> bool {
    if let toml::Value::Table(deps) = deps {
        deps.iter_mut().fold(false, |removed, (_, dep)| {
            if let toml::Value::Table(dep) = dep {
                if dep.remove("path").is_some() {
                    dep.entry("version")
                        .or_insert_with(|| toml::Value::String("*".into()));
                    return true;
                }
            }
            removed
        })
    } else {
        false
    }
}
