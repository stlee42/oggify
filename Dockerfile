FROM rust:1.46.0-slim-buster as rust
COPY / /oggify
WORKDIR /oggify
RUN cargo install --locked --path .

FROM scratch
COPY --from=rust /usr/local/cargo/bin/oggify /usr/local/bin/

CMD ["/usr/local/bin/oggify"]

