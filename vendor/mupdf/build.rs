fn main() {
    println!("cargo:rerun-if-changed=native");
    cc::Build::new()
        .include("native/include")
        .file("native/review.c")
        .compile("review_mupdf");
}
