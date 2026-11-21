//! Build script of `faset-upscale`.
//!
//! With the `dlss` feature, on Windows x86_64 (MSVC) only, it fetches the
//! parts of NVIDIA's NGX SDK that DLSS needs, generates their bindings with
//! bindgen and links the NGX static library. Elsewhere, or without the
//! feature, it does nothing: DLSS is then reported unavailable at runtime.
//!
//! The SDK is never in the repository. It is downloaded from NVIDIA's
//! repository at a pinned commit into `OUT_DIR`, and every file is checked
//! against its SHA-256. Set `NGX_SDK_DIR` to an SDK checkout of the same
//! version to build offline. Building with `dlss` on that target means using
//! the SDK under the NVIDIA RTX SDKs licence (`LICENSE.txt` of the SDK, copied
//! next to the binaries as `NVIDIA_RTX_SDK_LICENSE.txt`).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=ngx/wrapper.h");
    println!("cargo:rerun-if-env-changed=NGX_SDK_DIR");
    #[cfg(feature = "dlss")]
    {
        let target = |key: &str| std::env::var(key).unwrap_or_default();
        let supported = target("CARGO_CFG_TARGET_OS") == "windows"
            && target("CARGO_CFG_TARGET_ARCH") == "x86_64"
            && target("CARGO_CFG_TARGET_ENV") == "msvc";
        // DLSS is Windows x86_64 only: elsewhere nothing is fetched or linked.
        if supported {
            if let Err(e) = ngx_sdk::build() {
                panic!("feature `dlss`: {e}");
            }
        }
    }
}

#[cfg(feature = "dlss")]
mod ngx_sdk {
    use std::env;
    use std::error::Error;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// The DLSS SDK release, and the commit of github.com/NVIDIA/DLSS it tags.
    const SDK_VERSION: &str = "310.9.16";
    const SDK_COMMIT: &str = "9981c6350dc58c0ce21f024fd65e31e851c31689";
    const SDK_URL: &str = "https://raw.githubusercontent.com/NVIDIA/DLSS";

    /// The files used, as (path in the SDK, SHA-256).
    const FILES: [(&str, &str); 7] = [
        (
            "LICENSE.txt",
            "d4216e39ebef5f9b50a6712ebb37beeb5379862a67733a9999c651f21592aaf0",
        ),
        (
            "include/nvsdk_ngx_defs.h",
            "ea23f33497cd274860d1c25a97644fce807dcb0037c594547203343103fad03e",
        ),
        (
            "include/nvsdk_ngx_defs_vk.h",
            "0a24d0861ace7d6b9362a67b7f08bea1b33ea123c5ce730b19133de8a891d031",
        ),
        (
            "include/nvsdk_ngx_params.h",
            "943bc8cc5cdae03b6303016fbad3183636f2335ae27a2d18776798c3b4efabbc",
        ),
        (
            "include/nvsdk_ngx_vk.h",
            "2d364ce7132881eb669e9498fd570d74cba563b1fbebc82c235f8e1ad2dd8b6d",
        ),
        // NGX static library, dynamic CRT (/MD), like Rust's MSVC targets.
        (
            "lib/Windows_x86_64/x64/nvsdk_ngx_d.lib",
            "bc43df9ec032004fde681d3e3afd01cde239e03a0125b822e88674c84287f99c",
        ),
        // The DLSS runtime NGX loads from the executable's directory (release
        // build; the `dev` one is a watermarked developer tool).
        (
            "lib/Windows_x86_64/rel/nvngx_dlss.dll",
            "d00b5b0ab29c3ca101b70c3a5ecb3a01b7ef33766a3383a28a18623083e6123e",
        ),
    ];

    /// Files copied next to the binaries, as (path in the SDK, name there).
    const RUNTIME: [(&str, &str); 2] = [
        ("lib/Windows_x86_64/rel/nvngx_dlss.dll", "nvngx_dlss.dll"),
        ("LICENSE.txt", "NVIDIA_RTX_SDK_LICENSE.txt"),
    ];

    pub(crate) fn build() -> Result<(), Box<dyn Error>> {
        let out = PathBuf::from(env::var("OUT_DIR")?);
        let sdk = match env::var_os("NGX_SDK_DIR") {
            Some(dir) => {
                let dir = PathBuf::from(dir);
                for (path, sha) in FILES {
                    verify(&dir.join(path), sha)?;
                }
                dir
            }
            None => fetch(&out.join(format!("ngx-sdk-{SDK_VERSION}")))?,
        };
        bindings(&sdk, &out.join("ngx_bindings.rs"))?;
        println!(
            "cargo:rustc-link-search=native={}",
            sdk.join("lib/Windows_x86_64/x64").display()
        );
        println!("cargo:rustc-link-lib=static=nvsdk_ngx_d");
        copy_runtime(&sdk, &out)
    }

    /// Downloads the missing or stale SDK files into `dir`.
    fn fetch(dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
        let mut announced = false;
        for (path, sha) in FILES {
            let file = dir.join(path);
            if verify(&file, sha).is_ok() {
                continue;
            }
            if !announced {
                announced = true;
                println!(
                    "cargo:warning=downloading NVIDIA NGX SDK {SDK_VERSION} \
                     (NVIDIA RTX SDKs licence) for the `dlss` feature"
                );
            }
            if let Some(parent) = file.parent() {
                fs::create_dir_all(parent)?;
            }
            let url = format!("{SDK_URL}/{SDK_COMMIT}/{path}");
            let partial = file.with_extension("partial");
            let status = Command::new("curl")
                .args(["-fsSL", "--retry", "3", "-o"])
                .arg(&partial)
                .arg(&url)
                .status()
                .map_err(|e| format!("cannot run curl: {e}"))?;
            if !status.success() {
                return Err(format!("download of {url} failed ({status})").into());
            }
            verify(&partial, sha)?;
            fs::rename(&partial, &file)?;
        }
        Ok(dir.to_owned())
    }

    /// Fails unless `file` exists and has the SHA-256 `expected`.
    fn verify(file: &Path, expected: &str) -> Result<(), Box<dyn Error>> {
        use sha2::{Digest, Sha256};
        let bytes = fs::read(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        let digest = Sha256::digest(&bytes);
        let actual: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        if actual == expected {
            Ok(())
        } else {
            Err(format!(
                "{} has SHA-256 {actual}, expected {expected} (NGX SDK {SDK_VERSION})",
                file.display()
            )
            .into())
        }
    }

    /// Generates the Rust bindings of the NGX Vulkan API.
    fn bindings(sdk: &Path, out: &Path) -> Result<(), Box<dyn Error>> {
        let wrapper = Path::new(env!("CARGO_MANIFEST_DIR")).join("ngx/wrapper.h");
        let include = sdk.join("include");
        let bindings = bindgen::Builder::default()
            .header(wrapper.to_string_lossy())
            .clang_args([
                "-x",
                "c++",
                "-std=c++17",
                "-ffreestanding",
                "--target=x86_64-pc-windows-msvc",
            ])
            .clang_arg(format!("-I{}", include.display()))
            .allowlist_function(
                "NVSDK_NGX_VULKAN_(Init_with_ProjectID|Shutdown1|AllocateParameters|\
                 GetCapabilityParameters|DestroyParameters|CreateFeature1|ReleaseFeature|\
                 EvaluateFeature_C)",
            )
            .allowlist_function("NVSDK_NGX_Parameter_(SetI|SetUI|SetF|SetVoidPointer|GetI|GetUI)")
            .allowlist_type(
                "NVSDK_NGX_(Parameter|Handle|Resource_VK|Resource_VK_Type|ImageViewInfo_VK|\
                 Feature|Result|PerfQuality_Value|DLSS_Feature_Flags|EngineType|Version)",
            )
            .allowlist_var("NVSDK_NGX_Parameter_.*")
            // Dispatchable handles and entry points come from `ash`; the plain
            // types of the image resource (u64 handles, i32 format, the
            // subresource range) are generated so its union stays `Copy`.
            .blocklist_type("Vk(Instance|PhysicalDevice|Device|CommandBuffer)(_T)?|PFN_vk.*")
            .opaque_type("NVSDK_NGX_Parameter")
            .default_enum_style(bindgen::EnumVariation::Consts)
            .prepend_enum_name(false)
            .derive_default(false)
            .generate_comments(false)
            .raw_line(format!(
                "// Generated by build.rs (bindgen) from NGX SDK {SDK_VERSION}; never committed."
            ))
            .raw_line("pub type VkInstance = ash::vk::Instance;")
            .raw_line("pub type VkPhysicalDevice = ash::vk::PhysicalDevice;")
            .raw_line("pub type VkDevice = ash::vk::Device;")
            .raw_line("pub type VkCommandBuffer = ash::vk::CommandBuffer;")
            .raw_line(
                "pub type PFN_vkGetInstanceProcAddr = Option<ash::vk::PFN_vkGetInstanceProcAddr>;",
            )
            .raw_line(
                "pub type PFN_vkGetDeviceProcAddr = Option<ash::vk::PFN_vkGetDeviceProcAddr>;",
            )
            .generate()
            .map_err(|e| format!("bindgen failed on the NGX headers: {e}"))?;
        bindings.write_to_file(out)?;
        Ok(())
    }

    /// Copies the DLSS runtime and the SDK licence next to the binaries of
    /// this build (`target/<triple>/<profile>/` and its `examples/`), where
    /// NGX looks for `nvngx_dlss.dll`.
    fn copy_runtime(sdk: &Path, out: &Path) -> Result<(), Box<dyn Error>> {
        // OUT_DIR is <profile>/build/<package>-<hash>/out, or with the newer
        // build-dir layout <profile>/build/<package>/<hash>/out.
        let profile = out
            .ancestors()
            .find(|dir| dir.file_name().is_some_and(|name| name == "build"))
            .and_then(Path::parent);
        let Some(profile) = profile else {
            println!(
                "cargo:warning=cannot locate the output directory; copy nvngx_dlss.dll by hand"
            );
            return Ok(());
        };
        for dir in [profile.to_owned(), profile.join("examples")] {
            fs::create_dir_all(&dir)?;
            for (path, name) in RUNTIME {
                fs::copy(sdk.join(path), dir.join(name))?;
            }
        }
        Ok(())
    }
}
