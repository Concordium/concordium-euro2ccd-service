ARG ubuntu_version
ARG rust_image_tag

FROM rust:$rust_image_tag as builder

WORKDIR /build

COPY . .
RUN cargo build --release --locked

FROM ubuntu:$ubuntu_version

WORKDIR /build

COPY --from=builder /build/target/release/concordium-eur2ccd /build/concordium-eur2ccd

ENTRYPOINT [ "/build/concordium-eur2ccd" ]