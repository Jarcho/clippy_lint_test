# Clippy lint test

This is used to test clippy lints on the latest version of all crates currently in cargo's crates.io download cache.

## Usage

To run the lints use:

```sh
cargo run --bin clippy_lint_test CLIPPY_SRC_DIR -l LINT_NAME
```

This will generate a report file named `CLIPPY_BRANCH_NAME-DATE.txt` (name can be controlled with the `-r` flag). The report will include all diagnostic messages for the selected lints as well as a summary at the end.

## Downloading crates

Crates can be downloaded using:

```sh
cargo run --bin download_crates CRATES_IO_DATA_DUMP -n NUMBER_TO_DOWNLOAD
```

This will download the top `N` crates from crates.io as well as all their dependencies. The data dump can be downloaded [here](https://static.crates.io/db-dump.tar.gz).
