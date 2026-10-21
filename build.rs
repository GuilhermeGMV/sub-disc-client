fn main() {
    println!("cargo:rerun-if-changed=assets/kledisc.ico");
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("assets/kledisc.ico")
        .set("ProductName", "kledisc")
        .set("FileDescription", "kledisc")
        .compile()
        .expect("compile Windows application icon");
}
