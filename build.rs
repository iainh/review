fn main() {
    println!("cargo::rerun-if-changed=platform/windows/review.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("platform/windows/review.ico")
            .compile()
            .expect("failed to embed the Windows application icon");
    }
}
