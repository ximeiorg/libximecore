use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    #[cfg(target_os = "windows")]
    build_windows();

    #[cfg(not(target_os = "windows"))]
    build_unix();
}

fn librime_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    manifest_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("librime")
}

fn check_pkg_config() -> Option<PathBuf> {
    if pkg_config::find_library("rime").is_ok() {
        println!("cargo:warning=Found librime via pkg-config");
        println!("cargo:rerun-if-changed=build.rs");
        Some(PathBuf::new())
    } else {
        None
    }
}

fn cmake_build(librime: &std::path::Path) -> Option<PathBuf> {
    if !librime.join("CMakeLists.txt").exists() {
        return None;
    }

    println!(
        "cargo:warning=Building librime from submodule at {}",
        librime.display()
    );

    let dist_dir = librime.join("dist");
    if dist_dir.join("lib").join("librime.so").exists() {
        println!(
            "cargo:warning=Using cached librime build in {}",
            dist_dir.display()
        );
        return Some(dist_dir);
    }

    let build_dir = librime.join("build");

    let status = Command::new("cmake")
        .args([
            "-B",
            build_dir.to_str().unwrap(),
            "-S",
            librime.to_str().unwrap(),
            "-DCMAKE_BUILD_TYPE=Release",
            "-DBUILD_SHARED_LIBS=ON",
            "-DCMAKE_INSTALL_PREFIX=dist",
            "-DENABLE_EXTERNAL_PLUGINS=OFF",
            "-DENABLE_LOGGING=OFF",
        ])
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(_) => {
            println!("cargo:warning=cmake configure failed; install build deps:");
            println!("cargo:warning=  sudo apt install cmake build-essential libgoogle-glog-dev libyaml-cpp-dev");
            println!(
                "cargo:warning=  sudo dnf install cmake gcc-c++ google-glog-devel yaml-cpp-devel"
            );
            return None;
        }
        Err(e) => {
            println!(
                "cargo:warning=cmake not found: {}. Install cmake or system librime.",
                e
            );
            return None;
        }
    }

    let nproc = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    let status = Command::new("cmake")
        .args([
            "--build",
            build_dir.to_str().unwrap(),
            "--target",
            "install",
            "-j",
            &nproc.to_string(),
        ])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("cargo:warning=librime compilation finished successfully");
            Some(dist_dir)
        }
        Ok(_) => {
            println!("cargo:warning=cmake build failed; falling back to system librime hint");
            None
        }
        Err(e) => {
            println!("cargo:warning=cmake build failed: {}", e);
            None
        }
    }
}

fn link_librime(dist: Option<PathBuf>) {
    if let Some(dist_dir) = dist {
        if dist_dir.as_os_str().is_empty() {
            return;
        }
        let lib = dist_dir.join("lib");
        println!("cargo:rustc-link-search=native={}", lib.display());
        println!("cargo:rustc-link-lib=dylib=rime");
    } else {
        println!("cargo:rustc-link-lib=dylib=rime");
        if let Ok(lib_dir) = env::var("RIME_LIB_DIR") {
            println!("cargo:rustc-link-search=native={}", lib_dir);
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}

// ── Windows ──────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn build_windows() {
    use std::io::Write;
    use std::process::Stdio;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    let librime = librime_dir();
    let dist_dir = librime.join("dist");
    let dist_lib_dir = dist_dir.join("lib");
    let rime_dll = dist_lib_dir.join("rime.dll");

    if !rime_dll.exists() {
        build_librime_source_win(&librime, &dist_lib_dir);
    }

    if rime_dll.exists() {
        println!("cargo:rustc-link-search=native={}", dist_lib_dir.display());
        println!("cargo:rustc-link-lib=dylib=rime");
        let workspace_dir = librime.parent().unwrap();
        for profile in &["debug", "release"] {
            let target_dir = workspace_dir.join("target").join(profile);
            if target_dir.exists() {
                std::fs::copy(&rime_dll, target_dir.join("rime.dll")).ok();
            }
        }
    } else {
        panic!(
            "librime build failed: rime.dll not found at {}",
            rime_dll.display()
        );
    }
}

#[cfg(target_os = "windows")]
fn build_librime_source_win(librime_dir: &std::path::Path, _dist_lib_dir: &std::path::Path) {
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

    let workspace_dir = librime_dir.parent().unwrap();
    let vswhere = find_vswhere();
    let vs_install: String = match Command::new(&vswhere)
        .args(["-latest", "-property", "installationPath"])
        .output()
    {
        Ok(output) => String::from_utf8_lossy(&output.stdout).trim().to_string(),
        Err(e) => panic!("vswhere failed: {}", e),
    };

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
    let _ = std::fs::remove_file(&std::path::PathBuf::from(
        librime_dir.parent().unwrap().join("env.bat"),
    ));

    match status {
        Ok(s) if s.success() => println!("cargo:warning=librime compilation finished successfully"),
        Ok(s) => panic!("librime build failed with exit code: {:?}", s.code()),
        Err(e) => panic!("librime build failed to execute: {}", e),
    }
}

// ── Unix (Linux / macOS) ──────────────────────────────────────────────────

#[cfg(not(target_os = "windows"))]
fn build_unix() {
    let dist = check_pkg_config();
    if dist.is_some() {
        return link_librime(dist);
    }

    let librime = librime_dir();
    let dist = cmake_build(&librime);
    link_librime(dist);
}
