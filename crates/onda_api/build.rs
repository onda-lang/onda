fn main() {
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => {
            println!("cargo::rustc-link-arg-cdylib=-Wl,-soname,libonda.so");
        }
        Ok("macos") => {
            println!("cargo::rustc-link-arg-cdylib=-Wl,-install_name,@rpath/libonda.dylib");
        }
        _ => {}
    }
}
