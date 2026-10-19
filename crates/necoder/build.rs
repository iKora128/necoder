//! Windows の necoder.exe にアイコンと版情報を埋め込む（WINDOWS-PORT.md §W7）。
//!
//! - **アイコン（リソース ID 1）**: エクスプローラ・ショートカット・スタートメニューに出る絵。
//!   gpui はウィンドウ（タイトルバー・タスクバー・Alt+Tab）のアイコンも**同じ ID 1 を読む**
//!   （`gpui_windows` の `load_icon` が `LoadImageW(module, 1, IMAGE_ICON, …)`）ので、これ 1 つで全部に効く。
//!   無いと exe は汎用の白いアイコンになり、zip を開いた人がどれを起動すればよいか分からない
//! - **版情報（VERSIONINFO）**: エクスプローラのプロパティ・タスクマネージャの名前・SmartScreen の
//!   ダイアログに出る「アプリ名」。無いとタスクマネージャには `necoder.exe` とだけ出る
//!
//! mac / Linux では何もしない（.app の .icns は bundle-mac.sh が置く）。
//! gpui も `windows-manifest` で RT_MANIFEST（ID 1）を埋めるが、種類が違うので衝突しない。

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon/necoder.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("cargo が CARGO_MANIFEST_DIR を渡す"),
    );
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("cargo が OUT_DIR を渡す"));
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo が版を渡す");
    let icon = manifest_dir.join("assets").join("icon").join("necoder.ico");
    let rc = out_dir.join("necoder.rc");
    std::fs::write(&rc, resource_script(&icon, &version)).expect("OUT_DIR に .rc を書ける");
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_optional()
        .expect("アイコンと版情報を埋め込める（Windows SDK の rc.exe が要る）");
}

/// `1.2.3` → `1,2,3,0`（VERSIONINFO の数の版）。数でない部分（`-beta` 等）は 0。
fn numeric_version(version: &str) -> String {
    let mut parts: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    parts.resize(4, 0);
    parts
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn resource_script(icon: &std::path::Path, version: &str) -> String {
    // rc の文字列リテラルは `\` をエスケープとして読む。
    let icon = icon.display().to_string().replace('\\', "\\\\");
    let numeric = numeric_version(version);
    // 0x0411 = 日本語・0x04B0 = Unicode。表示名は製品名なので言語に依らず necoder。
    format!(
        r#"1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "041104B0"
    BEGIN
      VALUE "CompanyName", "necoder"
      VALUE "FileDescription", "necoder"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "necoder"
      VALUE "LegalCopyright", "AGPL-3.0-or-later"
      VALUE "OriginalFilename", "necoder.exe"
      VALUE "ProductName", "necoder"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x411, 1200
  END
END
"#
    )
}
