use std::{env, fs, path::PathBuf};

fn main() {
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    for name in ["tx", "actor"] {
        println!("cargo:rerun-if-changed={name}.ctl");
        let schema = fs::read_to_string(format!("{name}.ctl")).expect("CTL schema");
        let code = cells::ctl::compile(&schema).expect("valid CTL schema");
        fs::write(output.join(format!("{name}_wire.rs")), code)
            .expect("write generated Cell codecs");
    }
}
