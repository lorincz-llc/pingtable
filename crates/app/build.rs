//! Records build facts for the About box and, on Windows targets, gives the
//! executables an icon, version details and a manifest (per-monitor DPI, run
//! as the invoking user).

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets");
    println!("cargo:rerun-if-changed=build.rs");

    // Facts for the About box: the compiler version and what it compiled for.
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let rustc_version = std::process::Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|text| text.split_whitespace().nth(1).map(str::to_string))
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=PINGTABLE_RUSTC={rustc_version}");
    println!("cargo:rustc-env=PINGTABLE_TARGET={}", std::env::var("TARGET").unwrap_or_default());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let var = |name: &str| std::env::var(name).unwrap_or_default();
    let assets = PathBuf::from(var("CARGO_MANIFEST_DIR")).join("assets");
    // The resource compiler wants forward slashes, on every host.
    let asset = |file: &str| assets.join(file).display().to_string().replace('\\', "/");
    let (major, minor, patch) = (var("CARGO_PKG_VERSION_MAJOR"), var("CARGO_PKG_VERSION_MINOR"), var("CARGO_PKG_VERSION_PATCH"));
    let version = var("CARGO_PKG_VERSION");

    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "pingtable"
      VALUE "ProductName", "pingtable"
      VALUE "Comments", "hello@pingtable.net"
      VALUE "CompanyName", "Lorincz LLC"
      VALUE "LegalCopyright", "Copyright 2026 Lorincz LLC. Apache License 2.0."
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = asset("pingtable.ico"),
        manifest = asset("pingtable.manifest"),
    );
    let rc_path = PathBuf::from(var("OUT_DIR")).join("pingtable.rc");
    std::fs::write(&rc_path, rc).expect("write resource script");
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_required()
        .expect("compile Windows resources");
}
