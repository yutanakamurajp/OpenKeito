// The module is an extension loaded by Python: its Python symbols are
// resolved at load time (on macOS this needs `-undefined dynamic_lookup`).
fn main() {
    pyo3_build_config::add_extension_module_link_args();
}
