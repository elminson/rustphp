# syntax=docker/dockerfile:1

# ---- Build stage: compile the rustphp interpreter ----
FROM rust:1.95-slim AS build
WORKDIR /app
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

# ---- Runtime stage: tiny image with the interpreter + demo scripts ----
FROM debian:bookworm-slim
COPY --from=build /app/target/release/rustphp /usr/local/bin/rustphp
COPY examples /examples
WORKDIR /examples

# Expose a port for when rustphp is wired up as a CGI behind a web server.
EXPOSE 80

# By default, render the phpinfo() demo to stdout (parity with the original
# PHP 1.0 image, which ran `php php1.0-script.php`).
CMD ["/usr/local/bin/rustphp", "/examples/phpinfo.html"]
