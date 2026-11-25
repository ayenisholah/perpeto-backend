// `sqlx::migrate!` embeds the migrations at compile time; rebuild when one is
// added or edited, so a stale binary never reports "up to date".
fn main() {
    println!("cargo:rerun-if-changed=../../migrations");
}
