//! Diagnostic: print every Windows audio endpoint the MMDevice enumerator
//! reports, so a cable's real endpoint name can be compared with the name it
//! was exported under.
//!
//! ```text
//! cargo test -p owl-core --test cable_audio -- --ignored --nocapture print_endpoints
//! ```

use owl_core::get_system_audio_devices;

#[test]
#[ignore = "diagnostic helper, run explicitly"]
fn print_endpoints() {
    let devices = get_system_audio_devices().expect("enumerate devices");

    println!("\n=== RENDER (outputs) ===");
    for device in &devices.outputs {
        println!("  name={:?}\n    id={}", device.name, device.id);
    }

    println!("\n=== CAPTURE (inputs) ===");
    for device in &devices.inputs {
        println!("  name={:?}\n    id={}", device.name, device.id);
    }
    println!();
}
