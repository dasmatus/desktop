//! WebExtensions in Danube: Manifest V2 and V3 extensions as Chrome and
//! Firefox package them, installed with `danube install-extension`, served
//! at `chrome-extension://<id>/` and given `chrome.*` by Danube's own
//! runtime, since WPE WebKit ships only a manifest parser. `docs/danube.md`,
//! "Extensions", is the overview; each module here says its part.

pub mod inline;
pub mod manifest;
pub mod runtime;
pub mod store;
