FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev pkgconf openssl-dev openssl-libs-static
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY migrations ./migrations
RUN mkdir src && echo "fn main() {}" > src/main.rs \
    && cargo build --release \
    && rm -rf src
COPY src ./src
COPY res ./res
RUN touch src/main.rs && cargo build --release

FROM alpine:3.21
COPY --from=build /app/target/release/liner /usr/local/bin/
EXPOSE 3002
CMD ["liner", "serve"]
