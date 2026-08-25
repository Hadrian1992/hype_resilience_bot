fn main() {
    tonic_build::configure()
        .compile(&["proto/orderbook.proto"], &["proto"])
        .unwrap();
}
