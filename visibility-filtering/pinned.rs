use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::{env, fs};

pub(crate) fn assert_pinned(path: &str, rendered: &str) {
    let pinned = match fs::read_to_string(crate_path(path)) {
        Ok(pinned) => Some(pinned),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => panic!("{path}: {e}"),
    };
    if pinned.as_deref() == Some(rendered) {
        return;
    }
    write_output(path, rendered);
    panic!(
        "pinned file differs from what the test renders: run scripts/bless and review the diff\n{}",
        first_difference(path, pinned.as_deref(), rendered)
    );
}

pub(crate) fn crate_path(path: &str) -> PathBuf {
    let cargo = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    if cargo.exists() {
        cargo
    } else {
        Path::new("crates/x-product/xai-visibility-filtering-service").join(path)
    }
}

fn write_output(path: &str, rendered: &str) {
    let Some(outputs) = env::var_os("TEST_UNDECLARED_OUTPUTS_DIR") else {
        return;
    };
    let output = Path::new(&outputs).join("pinned").join(path);
    fs::create_dir_all(output.parent().unwrap()).unwrap();
    fs::write(output, rendered).unwrap();
}

fn first_difference(path: &str, pinned: Option<&str>, rendered: &str) -> String {
    let Some(pinned) = pinned else {
        return format!("{path}: missing");
    };
    let mut pinned_lines = pinned.split_inclusive('\n');
    let mut rendered_lines = rendered.split_inclusive('\n');
    let mut line = 1;
    loop {
        match (pinned_lines.next(), rendered_lines.next()) {
            (Some(p), Some(r)) if p == r => line += 1,
            (p, r) => {
                return format!(
                    "{path}: line {line}\n- {}\n+ {}",
                    p.map_or("(end of file)".to_owned(), |p| format!("{p:?}")),
                    r.map_or("(end of file)".to_owned(), |r| format!("{r:?}")),
                );
            }
        }
    }
}
