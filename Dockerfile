FROM rust:1.46.0-slim-buster as rust
COPY / /oggify
WORKDIR /oggify
RUN cargo build
RUN cargo install --path .
