//! Copy vendored ffmpeg/ffprobe into OUT_DIR when present for static bundling.

use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let out = Path::new(&out_dir);
    let target = env::var("TARGET").unwrap();
    let vendor = Path::new("vendor/ffmpeg").join(&target);
    println!("cargo:rerun-if-changed=vendor/ffmpeg");
    println!("cargo:rerun-if-changed=vendor/README.md");

    for tool in ["ffmpeg", "ffprobe"] {
        let src = vendor.join(tool);
        if src.is_file() {
            let dst = out.join(tool);
            fs::copy(&src, &dst).unwrap_or_else(|e| panic!("copy {}: {e}", src.display()));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&dst).unwrap().permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&dst, perms).unwrap();
            }
            println!("cargo:warning=bundled {tool} from {}", src.display());
        }
    }
}
