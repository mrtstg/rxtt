run:
	cargo run

install:
	cargo install --path .

build-static-glibc:
	RUSTFLAGS='-C target-feature=+crt-static' cargo build --release --target x86_64-unknown-linux-gnu

build-static-musl:
	cargo build --release --target x86_64-unknown-linux-musl

pack-release:
	tar -czvf rxtt-x86_64-unknown-linux-musl.tar.gz -C ./target/x86_64-unknown-linux-musl/release ./rxtt
	tar -czvf rxtt-x86_64-unknown-linux-gnu.tar.gz -C ./target/x86_64-unknown-linux-gnu/release ./rxtt
