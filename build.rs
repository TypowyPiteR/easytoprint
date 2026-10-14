//! Embeds assets/icon.ico into the Windows executable (exe file icon).

fn main() {
    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        println!("cargo:rerun-if-changed=assets/icon.rc");
        let out_dir = std::env::var("OUT_DIR").unwrap();
        let obj = std::path::Path::new(&out_dir).join("icon.o");
        let rc = "assets/icon.rc";
        let candidates = [
            "C:/msys64/mingw64/bin/windres.exe".to_string(),
            "windres".to_string(),
            "x86_64-w64-mingw32-windres".to_string(),
        ];
        let mut last_err = String::new();
        for windres in &candidates {
            match std::process::Command::new(windres).arg(rc).arg(&obj).output() {
                Ok(out) if out.status.success() => {
                    println!("cargo:rustc-link-arg={}", obj.display());
                    return;
                }
                Ok(out) => {
                    last_err = format!(
                        "windres failed: {}{}",
                        String::from_utf8_lossy(&out.stdout),
                        String::from_utf8_lossy(&out.stderr)
                    );
                }
                Err(e) => last_err = format!("could not run {windres}: {e}"),            }
        }
        panic!("embedding the icon failed — {last_err}");
    }
}
