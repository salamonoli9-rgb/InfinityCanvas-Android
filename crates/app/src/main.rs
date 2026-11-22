#[cfg(not(target_os = "android"))]
fn main() {
    infinite_canvas::desktop::run();
}

#[cfg(target_os = "android")]
fn main() {}
