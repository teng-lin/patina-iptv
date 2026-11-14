//! Linker setup for the native libmpv adapter.
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    println!("cargo:rerun-if-env-changed=IPTV_RS_MPVKIT");
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => macos(),
        Ok("ios") => ios(),
        _ => {}
    }
}

fn macos() {
    println!("cargo:rerun-if-env-changed=IPTV_RS_MPV_LIB_DIR");
    if let Ok(directory) = std::env::var("IPTV_RS_MPV_LIB_DIR") {
        assert!(
            Path::new(&directory).join("libmpv.dylib").is_file(),
            "missing pinned libmpv"
        );
        println!("cargo:rustc-link-search=native={directory}");
        return;
    }
    let pkg = std::process::Command::new("pkg-config")
        .args(["--variable=libdir", "mpv"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let dirs = pkg
        .into_iter()
        .chain(["/opt/homebrew/lib".to_owned(), "/usr/local/lib".to_owned()]);
    for dir in dirs {
        if Path::new(&dir).join("libmpv.dylib").exists() {
            println!("cargo:rustc-link-search=native={dir}");
            return;
        }
    }
    println!("cargo:warning=libmpv not found: install it with `brew install mpv`");
}

/// MPVKit's frameworks libmpv needs (MPVKit 1.0.0's `_MPVKit` and
/// `_FFmpeg` targets, LGPL).
const FRAMEWORKS: &[&str] = &[
    "Libmpv",
    "Libuchardet",
    "Libbluray",
    "Libavcodec",
    "Libavdevice",
    "Libavfilter",
    "Libavformat",
    "Libavutil",
    "Libswresample",
    "Libswscale",
    "Libssl",
    "Libcrypto",
    "Libass",
    "Libfreetype",
    "Libfribidi",
    "Libharfbuzz",
    "Libshaderc_combined",
    "lcms2",
    "Libplacebo",
    "Libdovi",
    "Libunibreak",
    "gmp",
    "nettle",
    "hogweed",
    "gnutls",
    "Libdav1d",
    "Libuavs3d",
];

/// The system's, as MPVKit's package links them (and OpenGL ES for the
/// view, MoltenVK's for its Vulkan).
const SYSTEM_FRAMEWORKS: &[&str] = &[
    "AVFoundation",
    "AudioToolbox",
    "CoreAudio",
    "CoreFoundation",
    "CoreMedia",
    "CoreVideo",
    "IOSurface",
    "Metal",
    "OpenGLES",
    "QuartzCore",
    "UIKit",
    "VideoToolbox",
];
const SYSTEM_LIBS: &[&str] = &["bz2", "iconv", "expat", "resolv", "xml2", "z", "c++"];

fn ios() {
    let simulator = std::env::var("TARGET").is_ok_and(|t| t.ends_with("-sim"));
    let slice = if simulator {
        "ios-arm64_x86_64-simulator"
    } else {
        "ios-arm64"
    };
    let root = std::env::var_os("IPTV_RS_MPVKIT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
            manifest.join("../../target/mpvkit")
        });
    if !root.join("Libmpv.xcframework").exists() {
        println!(
            "cargo:warning=MPVKit not found in {}: run tools/fetch-mpvkit.sh",
            root.display()
        );
        return;
    }
    for name in FRAMEWORKS {
        let dir = root.join(format!("{name}.xcframework")).join(slice);
        println!("cargo:rerun-if-changed={}", dir.display());
        println!("cargo:rustc-link-search=framework={}", dir.display());
        println!("cargo:rustc-link-lib=framework={name}");
    }
    let molten = root.join("MoltenVK.xcframework").join(slice);
    println!("cargo:rustc-link-search=native={}", molten.display());
    // A universal archive: the linker takes it, rustc could not bundle it.
    println!("cargo:rustc-link-lib=static:-bundle=MoltenVK");
    for name in SYSTEM_FRAMEWORKS {
        println!("cargo:rustc-link-lib=framework={name}");
    }
    for name in SYSTEM_LIBS {
        println!("cargo:rustc-link-lib=dylib={name}");
    }
}
