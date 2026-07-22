//! Pure rendering of a Signal device-link URI to terminal output (#921/#922).
//!
//! [`render_link`] is an **I/O-free, deterministic** function: given whatever
//! URI signal-cli emits (modern `sgnl://linkdevice?...` or legacy
//! `tsdevice:/?...`), it returns a block-glyph QR code with the **raw URI
//! printed verbatim underneath** as a copy/paste fallback. Keeping it pure
//! makes it fully unit-testable with no terminal, clock, or process.

use qrcode::QrCode;
use qrcode::render::unicode;

/// Render a device-link URI as a scannable terminal QR code followed by the
/// raw URI as a fallback. Never panics: if QR encoding fails (e.g. the URI is
/// too large to encode), the raw URI is still returned with a note so the
/// operator can always link by other means.
pub fn render_link(uri: &str) -> String {
    let mut out = String::new();
    match QrCode::new(uri.as_bytes()) {
        Ok(code) => {
            let qr = code
                .render::<unicode::Dense1x2>()
                .dark_color(unicode::Dense1x2::Dark)
                .light_color(unicode::Dense1x2::Light)
                .quiet_zone(true)
                .build();
            out.push_str(&qr);
            out.push('\n');
        }
        Err(err) => {
            out.push_str(&format!(
                "(could not render QR code: {err}; use the URI below)\n"
            ));
        }
    }
    out.push('\n');
    out.push_str("Scan this in Signal → Settings → Linked devices → Link new device.\n");
    out.push_str("If the QR will not scan, paste this device-link URI instead:\n");
    out.push_str(uri);
    out.push('\n');
    out
}
