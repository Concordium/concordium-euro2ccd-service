FROM rust as rust-builder
WORKDIR /app

# Use cached dependencies builds as long as cargo files do not change
COPY ./Cargo.toml .
COPY ./Cargo.lock .
RUN mkdir -p ./src && echo 'fn main() { println!("Dummy!"); }' > ./src/main.rs
RUN cargo build --release
# Remove dummy print
RUN rm -rf ./src
COPY ./src ./src
RUN touch -a -m ./src/main.rs
# Build actual binary
RUN cargo build --release

FROM debian
COPY --from=rust-builder /app/target/release/eur2ccd-service-test-exchange /usr/local/bin/
WORKDIR /app
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get -y install debhelper dh-exec
ENTRYPOINT ["/usr/local/bin/eur2ccd-service-test-exchange"]