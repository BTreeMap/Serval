# syntax=docker/dockerfile:1

# ---- Stage 1: build the embedded frontend ----------------------------------
FROM node:24-slim AS frontend
WORKDIR /app/frontend

# Install dependencies against the lockfile first for layer caching.
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci

# Build the production bundle into frontend/dist.
COPY frontend/ ./
RUN npm run build

# ---- Stage 2: build the Rust binary ----------------------------------------
FROM rust:1.96-slim-bookworm AS backend
WORKDIR /app

# Cache dependency compilation: copy manifests, then fetch.
COPY Cargo.toml Cargo.lock rust-toolchain.toml build.rs ./
RUN mkdir -p src benches && echo "fn main() {}" > src/main.rs \
    && echo "" > src/lib.rs \
    && echo "fn main() {}" > benches/data_plane_hot_path.rs \
    && mkdir -p frontend/dist && echo "<!doctype html>" > frontend/dist/index.html \
    && SERVAL_SKIP_FRONTEND_BUILD=1 cargo build --release --locked || true
RUN rm -rf src benches

# Build for real against the source and the prebuilt frontend.
COPY src ./src
COPY benches ./benches
COPY --from=frontend /app/frontend/dist ./frontend/dist
ENV SERVAL_SKIP_FRONTEND_BUILD=1
RUN cargo build --release --locked

# ---- Stage 3: minimal, non-root runtime ------------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=backend /app/target/release/serval /usr/local/bin/serval

# Control Plane (8080) and Data Plane (3000).
EXPOSE 8080 3000
USER nonroot
ENTRYPOINT ["/usr/local/bin/serval"]
CMD ["serve"]
