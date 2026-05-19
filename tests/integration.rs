use std::{fs, path::Path};

use assert_cmd::Command;
use tempfile::TempDir;

#[test]
fn allow_no_vcs_flag() {
    let tempdir = fixture_tempdir();

    remove_unused_derives(tempdir.path())
        .arg("--write")
        .assert()
        .code(2)
        .failure();
}

#[test]
fn allow_dirty_flag() {
    let tempdir = fixture_tempdir();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(tempdir.path())
        .status()
        .unwrap();

    remove_unused_derives(tempdir.path())
        .arg("--write")
        .assert()
        .code(2)
        .failure();
}

#[test]
fn removes_unused_derives() {
    let tempdir = fixture_tempdir();
    remove_unused_derives(tempdir.path())
        .arg("--write")
        .arg("--allow-no-vcs")
        .assert()
        .success();

    Command::new("cargo")
        .arg("check")
        .current_dir(tempdir.path())
        .assert()
        .success();

    let alpha_lib = fs::read_to_string(tempdir.path().join("alpha/src/lib.rs")).unwrap();
    let alpha_shared = fs::read_to_string(tempdir.path().join("alpha/src/shared.rs")).unwrap();
    let beta_lib = fs::read_to_string(tempdir.path().join("beta/src/lib.rs")).unwrap();
    let gamma_lib = fs::read_to_string(tempdir.path().join("gamma/src/lib.rs")).unwrap();

    assert!(derives_for(&alpha_lib, "Untouched").is_empty());
    assert_eq!(derives_for(&alpha_lib, "PartiallyUsed"), vec!["Clone"]);
    assert_eq!(derives_for(&alpha_lib, "AllUsed"), vec!["Debug", "Clone"]);
    assert_eq!(
        derives_for(&alpha_lib, "Kind"),
        vec!["PartialEq", "Eq", "Hash"]
    );
    assert_eq!(derives_for(&beta_lib, "Beta"), vec!["Debug", "Clone"]);
    assert_eq!(derives_for(&gamma_lib, "User"), vec!["Deserialize"]);
    assert_eq!(derives_for(&gamma_lib, "Op"), vec!["Clone", "Copy"]);
    assert_eq!(
        derives_for(&alpha_shared, "HandlerParams"),
        vec!["Debug", "Clone", "Deserialize"]
    );
}

fn fixture_tempdir() -> TempDir {
    let tempdir = TempDir::new().unwrap();
    let fixture_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace");
    copy_dir(&fixture_src, tempdir.path()).unwrap();
    tempdir
}

fn remove_unused_derives(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("cargo-remove-unused-derives").unwrap();
    cmd.arg("remove-unused-derives").current_dir(dir);
    cmd
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn derives_for(content: &str, item_name: &str) -> Vec<String> {
    let file = syn::parse_file(content).expect("failed to parse file");
    for item in &file.items {
        let attrs = match item {
            syn::Item::Struct(s) if s.ident == item_name => &s.attrs,
            syn::Item::Enum(e) if e.ident == item_name => &e.attrs,
            _ => continue,
        };
        let mut derives = Vec::new();
        for attr in attrs {
            if attr.path().is_ident("derive") {
                attr.parse_nested_meta(|meta| {
                    if let Some(ident) = meta.path.get_ident() {
                        derives.push(ident.to_string());
                    }
                    Ok(())
                })
                .unwrap();
            }
        }
        return derives;
    }
    panic!("item {item_name} not found");
}
