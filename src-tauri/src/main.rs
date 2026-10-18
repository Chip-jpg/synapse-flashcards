// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use synapse_scaffold_lib::extract;

fn main() {
    // Started by Synapse itself to read a PDF's text (see `extract.rs`): do
    // only that, without opening a window.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == extract::PDF_HELPER_ARG)
    {
        std::process::exit(extract::pdf_helper());
    }
    synapse_scaffold_lib::run()
}
