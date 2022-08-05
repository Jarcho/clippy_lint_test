use anyhow::{Context, Result};
use clippy_lint_test::{is_rustc_crate, LatestVersions, Version};
use csv::{ReaderBuilder, StringRecord};
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};
use temp_dir::TempDir;

#[derive(argh::FromArgs)]
/// Download the top crates into cargo's crate cache
struct Args {
    /// path containing the crates.io data dump
    #[argh(positional)]
    dump_path: PathBuf,

    /// the number of crates to download
    #[argh(option, short = 'n')]
    count: Option<usize>,
}

fn main() -> Result<()> {
    let args: Args = argh::from_env();

    let mut crates = read_crates(&args.dump_path, read_versions(&args.dump_path));

    crates.sort_by(|x, y| x.download_count.cmp(&y.download_count).reverse());

    let dir = TempDir::new().context("error creating temp dir")?;
    let temp_path = dir.path();

    fs::create_dir(temp_path.join("src")).context("error creating item in temp dir")?;
    fs::File::create(temp_path.join("src").join("lib.rs"))
        .context("error creating item in temp dir")?;
    let toml_path = temp_path.join("Cargo.toml");
    let cargo_home =
        home::cargo_home_with_cwd(temp_path).context("error getting cargo home dir")?;
    let crates_io_cache = cargo_home
        .join("registry")
        .join("cache")
        .join("github.com-1ecc6299db9ec823");

    let crates = crates.get(..args.count.unwrap_or(500)).unwrap_or(&crates);
    // Dependencies likely have more downloads than dependant crates. Download in reverse order.
    for (i, c) in crates.iter().rev().enumerate() {
        for version in c.versions.versions() {
            if crates_io_cache
                .join(format!("{}-{}.crate", c.name, version))
                .exists()
            {
                print!("{}/{}\r", i, crates.len());
                let _ = io::stdout().flush();
                continue;
            }

            println!("fetching `{}-{}`", c.name, version);
            print!("{}/{}\r", i, crates.len());
            let _ = io::stdout().flush();

            let mut toml_file = fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&toml_path)
                .context("error creating item in temp dir")?;

            write!(
                toml_file,
                "[package]
                name = \"package\"
                version = \"0.1.0\"

                [dependencies]
                {} = \"{}\"
                ",
                c.name, version
            )
            .context("error writing item in temp dir")?;

            drop(toml_file);
            if !Command::new("cargo")
                .arg("fetch")
                .current_dir(temp_path)
                .output()
                .unwrap()
                .status
                .success()
            {
                eprintln!("error fetching dependencies for `{}-{}`", c.name, version);
            }
        }
    }

    Ok(())
}

struct Crate {
    name: String,
    download_count: u64,
    versions: LatestVersions,
}

fn read_versions(p: &Path) -> HashMap<u64, LatestVersions> {
    let mut csv = ReaderBuilder::new()
        .has_headers(true)
        .from_path(p.join("versions.csv"))
        .expect("error reading versions.csv");

    let headers = ["crate_id", "num", "yanked"];
    let indicies = headers_to_indicies(csv.headers().expect("error reading file header"), headers);
    let mut result = HashMap::<_, LatestVersions>::new();
    for r in csv.into_records() {
        let r = r.expect("error reading record");
        let data = extract_indicies(&r, indicies);
        if data[2] == "t" {
            continue;
        }
        let id = data[0].parse().expect("error parsing crate id");
        if let Some(version) = Version::parse(data[1]) {
            result.entry(id).or_default().push(version);
        }
    }
    result
}

fn read_crates(p: &Path, mut versions: HashMap<u64, LatestVersions>) -> Vec<Crate> {
    let mut csv = ReaderBuilder::new()
        .has_headers(true)
        .from_path(p.join("crates.csv"))
        .expect("error reading crates.csv");

    let headers = ["downloads", "id", "name"];
    let indicies = headers_to_indicies(csv.headers().expect("error reading file header"), headers);
    csv.into_records()
        .filter_map(|r| {
            let r = r.expect("error reading record");
            let data = extract_indicies(&r, indicies);
            if is_rustc_crate(data[2]) {
                return None;
            }
            let download_count = data[0].parse().expect("error parsing crate id");
            let id = data[1].parse().expect("error parsing crate id");
            let name = data[2].into();
            let versions = versions.remove(&id)?;
            Some(Crate {
                name,
                download_count,
                versions,
            })
        })
        .collect()
}

fn headers_to_indicies<const N: usize>(r: &StringRecord, headers: [&'static str; N]) -> [usize; N] {
    let mut found = [None; N];
    for (i, field) in r.iter().enumerate() {
        if let Some(which) = headers.iter().position(|&h| h == field) {
            found[which] = Some(i);
        }
    }
    found.map(|x| x.expect("failed to find header"))
}

fn extract_indicies<const N: usize>(r: &StringRecord, indicies: [usize; N]) -> [&str; N] {
    let mut found = [None; N];
    for (i, field) in r.iter().enumerate() {
        if let Some(which) = indicies.iter().position(|&index| index == i) {
            found[which] = Some(field);
        }
    }
    found.map(|x| x.expect("failed to find header value"))
}
