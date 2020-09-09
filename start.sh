cd client
wasm-pack build --target web --debug -d ~/quoridor_web/static/pkg
cd ../
cargo run
