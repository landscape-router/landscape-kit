//! Compile on the CI host with rustc; use the product renderer, not a copied
//! OpenRC script, for the VM's minimal Landscape process fixture.
#[allow(dead_code)]
#[path = "../../../lkit-cli/src/service/openrc_definition.rs"]
mod definition;

fn main() {
    let output = std::env::args_os().nth(1).expect("output path");
    std::fs::write(
        output,
        definition::render_router_script(std::path::Path::new("/srv/landscape")),
    )
    .unwrap();
}
