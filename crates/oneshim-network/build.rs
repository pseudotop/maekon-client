fn main() {
    // Proto generated code is committed to git (src/proto/generated/).
    // To regenerate after .proto changes, run: scripts/regenerate-protos.sh
    //
    // Register proto files for cargo to watch — triggers rebuild if changed.
    let proto_dir = "../../api/proto/oneshim/client/v1";
    for name in ["auth", "session", "context", "suggestion", "health"] {
        println!("cargo:rerun-if-changed={proto_dir}/{name}.proto");
    }
}
