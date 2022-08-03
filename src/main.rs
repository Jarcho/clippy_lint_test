use anyhow::{bail, Context, Result};
use cargo_metadata::{
    diagnostic::{Diagnostic, DiagnosticLevel},
    CompilerMessage, Message,
};
use clippy_lint_test::Version;
use flate2::read::GzDecoder;
use rm_rf::remove;
use std::{
    collections::hash_map::{Entry, HashMap},
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

    /// the name of the report file, defaults to `CLIPPY_BRANCH_NAME-CURRENT_DATE.txt`
    #[argh(option, short = 'r', long = "report-file")]
    report_name: Option<PathBuf>,

    /// lints to test
    #[argh(option, short = 'l', long = "lint")]
    lints: Vec<String>,
}

fn main() -> Result<()> {
    let args: Args = argh::from_env();
    let temp_dir = temp_dir::TempDir::new().expect("error creating temp dir");
    let temp_dir = temp_dir.path();

    let home_dir = home::cargo_home().context("error finding cargo home dir")?;
    let crates_dir = home_dir
        .join("registry")
        .join("cache")
        .join("github.com-1ecc6299db9ec823");
    let crates = find_crates(&crates_dir)?;

    println!("Compiling clippy...");
    let clippy_args = compile_clippy(&args.clippy_dir)?;
    let target_dir = temp_dir.join("target");
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
    let mut per_crate_count = HashMap::new();
    let mut report = io::BufWriter::new(
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(args.report_name.unwrap_or_else(|| {
                let res = Command::new("git")
                    .args(["branch", "--show_current"])
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
    for (i, c) in crates.values().enumerate() {
        if i > 0 && i % 256 == 0 {
            // Don't let the target directory get too big.
            let _ = remove(&target_dir);
        }

        println!("Checking crate `{}`...", c.contained_name);
        print!("{}/{}\r", i, crates.len());
        let _ = io::stdout().flush();
        match check_crate(&clippy_args, &target_dir, &mut lint_counters, c, temp_dir) {
            Ok(messages) if !messages.is_empty() => {
                per_crate_count.insert(c.contained_name.clone(), messages.len());
                println!("Found {} warnings", messages.len());
                write!(
                    report,
                    "{}: {} warnings\n\n",
                    c.contained_name,
                    messages.len()
                )
                .context("error writing report")?;
                for m in messages {
                    report
                        .write_all(m.as_bytes())
                        .context("error writing report")?;
                }
                writeln!(report).context("error writing report")?;
                report.flush().context("error writing report")?;
            }
            Ok(_) => (),
            Err(e) => eprintln!("error checking crate `{}`: {}", c.contained_name, e),
        }
    }

    write!(report, "\nReport summary:\n\n").context("error writing report")?;
    for (krate, count) in per_crate_count {
        writeln!(report, "{}: {} warnings", krate, count).context("error writing report")?
    }
    writeln!(report).context("error writing report")?;
    for (lint, count) in lint_counters {
        writeln!(report, "{}: {} occurrences", lint, count).context("error writing report")?;
    }
    report.flush().context("error writing report")?;

    let _ = remove(&target_dir);
    Ok(())
}

struct CrateFileName<'a> {
    crate_name: &'a str,
    version: Version,
    contained_name: &'a str,
}
fn parse_crate_name(path: &Path) -> Option<CrateFileName<'_>> {
    let stem = path.file_stem()?.to_str()?;
    let (crate_name, version) = stem.rsplit_once('-')?;
    Some(CrateFileName {
        crate_name,
        version: version.parse().ok()?,
        contained_name: stem,
    })
}

struct CrateInfo {
    path: PathBuf,
    version: Version,
    contained_name: String,
}

fn find_crates(p: &Path) -> Result<HashMap<String, CrateInfo>> {
    let mut crates = HashMap::new();
    for file in fs::read_dir(p).with_context(|| format!("error reading dir `{}`", p.display()))? {
        let file = file.with_context(|| format!("error reading dir `{}`", p.display()))?;
        let path = file.path();
        if let Some(name) = parse_crate_name(&path) {
            match crates.entry(name.crate_name.into()) {
                Entry::Vacant(e) => {
                    e.insert(CrateInfo {
                        version: name.version,
                        contained_name: name.contained_name.into(),
                        path,
                    });
                }
                Entry::Occupied(mut e) if name.version > e.get().version => {
                    e.insert(CrateInfo {
                        version: name.version,
                        contained_name: name.contained_name.into(),
                        path,
                    });
                }
                Entry::Occupied(_) => (),
            }
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
        let args: [&OsStr; 8] = [
            self.channel.as_ref(),
            "--quiet".as_ref(),
            "run".as_ref(),
            &self.manifest,
            "--release".as_ref(),
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

    let args: [&OsStr; 4] = [
        channel_arg.as_ref(),
        "build".as_ref(),
        &manifest_arg,
        "--release".as_ref(),
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

fn check_crate(
    clippy_args: &ClippyArgs,
    target_dir: &Path,
    lints: &mut HashMap<String, usize>,
    krate: &CrateInfo,
    temp_dir: &Path,
) -> Result<Vec<String>> {
    extract_crate(&krate.path, temp_dir)?;
    let path = temp_dir.join(&krate.contained_name);
    let _delayed = RemoveOnDrop(&path);
    remove_file(&path.join(".cargo").join("config"))?;
    remove_file(&path.join("Cargo.lock"))?;
    let manifest_path = path.join("Cargo.toml");
    prepare_manifest(&manifest_path)?;

    let args: [&OsStr; 14] = [
        "--".as_ref(),
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
    if !output.status.success() {
        let mut msg = format!("error running clippy({})\n", output.status);
        for message in Message::parse_stream(output.stdout.as_slice())
            .filter_map(|m| {
                let m = match m.context("error parsing `cargo` output") {
                    Ok(m) => m,
                    Err(e) => return Some(Err(e)),
                };
                if let Message::CompilerMessage(CompilerMessage {
                    message:
                        Diagnostic {
                            rendered: Some(rendered),
                            level: DiagnosticLevel::Error | DiagnosticLevel::Ice,
                            ..
                        },
                    ..
                }) = m
                {
                    Some(Ok(rendered))
                } else {
                    None
                }
            })
            .collect::<Result<Vec<_>, _>>()?
        {
            msg.push_str(&message);
        }
        msg.push_str(
            str::from_utf8(&output.stderr).context("error converting `cargo` output to `str`")?,
        );
        bail!(msg);
    }

    Message::parse_stream(output.stdout.as_slice())
        .filter_map(|m| {
            let m = match m.context("error parsing `cargo` output") {
                Ok(m) => m,
                Err(e) => return Some(Err(e)),
            };
            if let Message::CompilerMessage(CompilerMessage {
                message:
                    Diagnostic {
                        code: Some(code),
                        rendered: Some(rendered),
                        ..
                    },
                ..
            }) = m
            {
                if let Some(count) = lints.get_mut(&code.code) {
                    *count += 1;
                    Some(Ok(rendered))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect()
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
