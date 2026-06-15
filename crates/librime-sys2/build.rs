use std::env;

fn main() {
    #[cfg(target_os = "windows")]
    build_windows();

    #[cfg(not(target_os = "windows"))]
    build_unix();
}

#[cfg(target_os = "windows")]
fn build_windows() {
    use std::io::Write;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_dir = manifest_dir.parent().unwrap().parent().unwrap();
    let librime_dir = workspace_dir.join("librime");
    let dist_dir = librime_dir.join("dist");
    let dist_lib_dir = dist_dir.join("lib");
    let rime_dll = dist_lib_dir.join("rime.dll");

    if !rime_dll.exists() {
        build_librime_source(&librime_dir, &workspace_dir);
    }

    if rime_dll.exists() {
        println!("cargo:rustc-link-search=native={}", dist_lib_dir.display());
        println!("cargo:rustc-link-lib=dylib=rime");
        for profile in &["debug", "release"] {
            let target_dir = workspace_dir.join("target").join(profile);
            let target_rime_dll = target_dir.join("rime.dll");
            if target_dir.exists() {
                std::fs::copy(&rime_dll, &target_rime_dll).ok();
            }
        }
    } else {
        panic!(
            "librime build failed: rime.dll not found at {}",
            rime_dll.display()
        );
    }

    println!("cargo:rerun-if-changed=build.rs");
}

#[cfg(target_os = "windows")]
fn build_librime_source(librime_dir: &std::path::Path, workspace_dir: &std::path::Path) {
    use std::io::Write;
    use std::path::PathBuf;
    use std::process::Command;

    println!(
        "cargo:warning=Building librime from source (this may take 10+ minutes on first build)..."
    );

    fn find_vswhere() -> PathBuf {
        if let Ok(output) = Command::new("where").arg("vswhere").output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return PathBuf::from(path);
                }
            }
        }
        for candidate in [
            r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe",
            r"C:\Program Files\Microsoft Visual Studio\Installer\vswhere.exe",
        ] {
            let p = PathBuf::from(candidate);
            if p.exists() {
                return p;
            }
        }
        panic!("vswhere not found. Install Visual Studio 2022.");
    }

    let vswhere = find_vswhere();
    let vs_install: String = match Command::new(&vswhere)
        .args(["-latest", "-property", "installationPath"])
        .output()
    {
        Ok(output) => String::from_utf8_lossy(&output.stdout).trim().to_string(),
        Err(e) => panic!("vswhere failed: {}", e),
    };

    let env_bat = librime_dir.join("env.bat");
    {
        let mut file = std::fs::File::create(&env_bat).unwrap();
        writeln!(file, "@echo off").unwrap();
        writeln!(file, "set RIME_ROOT={}", librime_dir.display()).unwrap();
        writeln!(
            file,
            "if not defined BOOST_ROOT set BOOST_ROOT={}\\deps\\boost-1.89.0",
            librime_dir.display()
        )
        .unwrap();
        writeln!(file, "set ARCH=x64").unwrap();
        writeln!(file, "set BJAM_TOOLSET=msvc-14.3").unwrap();
        writeln!(file, "set CMAKE_GENERATOR=\"Visual Studio 17 2022\"").unwrap();
        writeln!(file, "set PLATFORM_TOOLSET=v143").unwrap();
    }

    let temp_bat = workspace_dir.join("temp-build-librime.bat");
    {
        let mut file = std::fs::File::create(&temp_bat).unwrap();
        writeln!(file, "@echo off").unwrap();
        writeln!(
            file,
            "call \"{}\\VC\\Auxiliary\\Build\\vcvars64.bat\"",
            vs_install
        )
        .unwrap();
        writeln!(file, "cd /d \"{}\"", librime_dir.display()).unwrap();
        writeln!(
            file,
            "if not exist \"{0}\\deps\\boost-1.89.0\\boost\" call install-boost.bat",
            librime_dir.display()
        )
        .unwrap();
        writeln!(
            file,
            "if not defined BOOST_ROOT set BOOST_ROOT={}\\deps\\boost-1.89.0",
            librime_dir.display()
        )
        .unwrap();
        writeln!(file, "build.bat deps librime shared").unwrap();
    }

    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();
    let progress_thread = thread::spawn(move || {
        let mut count = 0;
        while running_clone.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_secs(30));
            if running_clone.load(Ordering::Relaxed) {
                count += 1;
                println!(
                    "cargo:warning=librime build in progress... ({} minutes elapsed)",
                    count / 2
                );
            }
        }
    });

    let status = Command::new(&temp_bat)
        .current_dir(workspace_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    running.store(false, Ordering::Relaxed);
    progress_thread.join().ok();
    std::fs::remove_file(&temp_bat).ok();

    match status {
        Ok(s) if s.success() => {
            println!("cargo:warning=librime compilation finished successfully");
        }
        Ok(s) => panic!("librime build failed with exit code: {:?}", s.code()),
        Err(e) => panic!("librime build failed to execute: {}", e),
    }
}

#[cfg(not(target_os = "windows"))]
fn build_unix() {
    if pkg_config::find_library("rime").is_ok() {
        println!("cargo:warning=Found librime via pkg-config");
    } else {
        println!("cargo:rustc-link-lib=dylib=rime");
        if let Ok(lib_dir) = env::var("RIME_LIB_DIR") {
            println!("cargo:rustc-link-search=native={}", lib_dir);
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
