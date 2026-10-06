//! Embeds the release bundles and compiles the icon/manifest resource.
use std::path::Path;
use std::{env, fs, process::Command};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let out = env::var("OUT_DIR").unwrap();
    let mut files = vec![
        ("lumen-windows.ps1".to_string(), root.join("install/lumen-windows.ps1")),
        ("lumen.ico".to_string(), root.join("assets/lumen.ico")),
    ];
    for arch in ["x86_64", "aarch64"] {
        let dir = root.join("dist").join(arch);
        let mut names: Vec<_> = fs::read_dir(&dir)
            .unwrap_or_else(|_| panic!("{} missing: run tools/dist.sh {arch} first", dir.display()))
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.ends_with(".efi") || n.ends_with(".cer") || n.ends_with(".conf"))
            .collect();
        names.sort();
        for n in names {
            files.push((format!("{arch}/{n}"), dir.join(&n)));
        }
    }
    // Lumen for legacy BIOS PCs (x64 only).
    for n in ["lumen-bios.img", "lumen-bios-install.exe"] {
        let path = root.join("dist/x86_64/bios").join(n);
        assert!(path.exists(), "{} missing: run tools/dist.sh x86_64 first", path.display());
        files.push((format!("x86_64/bios/{n}"), path));
    }
    let mut src = String::new();
    for (name, path) in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        src += &format!("({name:?}, include_bytes!({:?}) as &[u8]),\n", path.display().to_string());
    }
    fs::write(Path::new(&out).join("files.rs"), format!("&[{src}]")).unwrap();

    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=app.manifest");
    let res = Path::new(&out).join("app-res.o");
    let windres = env::var("WINDRES").unwrap_or_else(|_| "x86_64-w64-mingw32-windres".into());
    let ok = Command::new(&windres)
        .args(["app.rc", "-O", "coff", "-o"])
        .arg(&res)
        .status()
        .unwrap_or_else(|e| panic!("couldn't run {windres}: {e}"));
    assert!(ok.success(), "windres failed");
    println!("cargo:rustc-link-arg-bins={}", res.display());
}
