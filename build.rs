fn main() {
    println!("cargo:rerun-if-changed=assets/icons/veilkey.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/icons/veilkey.ico")
            .compile()
            .expect("failed to embed the VeilKey application icon");
    }
}
