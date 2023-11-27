FROM rust:1.68 as builder

WORKDIR /build

COPY . .
RUN cargo build --release

FROM ubuntu:20.04

WORKDIR /build

COPY --from=builder /build/target/release/concordium-eur2ccd /build/concordium-eur2ccd

ENTRYPOINT [ "/build/concordium-eur2ccd" ]