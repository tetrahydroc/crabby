//! Print the assembled Lib source to stdout. Used to inspect the
//! Lib fragments concat (lib.gd + boot.gd + every registry/*.gd).
//!
//! ```sh
//! cargo run --example dump_shim -p crabby-install > /tmp/assembled.gd
//! ```

use crabby_install::LIB_SOURCE;

fn main() {
    print!("{}", &*LIB_SOURCE);
}
