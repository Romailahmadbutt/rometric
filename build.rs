fn main() {
    println!("cargo:rerun-if-changed=assets/logo.ico");

    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/logo.ico");
        resource.set("ProductName", "Rometric");
        resource.set("FileDescription", "Rometric Signal Bench");
        resource
            .compile()
            .expect("Windows application icon must compile");
    }
}
