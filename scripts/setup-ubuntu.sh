#!/usr/bin/env bash
# vw の開発環境を Ubuntu に揃える。
#
#   ./scripts/setup-ubuntu.sh            # 必要なパッケージと Rust を入れて、ビルドとテストまで行う
#   ./scripts/setup-ubuntu.sh --no-build # 環境だけ揃える
#
# 何度実行しても問題ない（入っているものは飛ばす）。
set -euo pipefail

build=1
[[ "${1:-}" == "--no-build" ]] && build=0

cd "$(dirname "$0")/.."

echo "==> apt パッケージ"
# ビルドに必要なのはコンパイラ類だけ。X11 / Wayland / OpenGL は実行時に読み込むので、
# 実行に必要なライブラリを入れておく（デスクトップ版 Ubuntu なら大半は入っている）。
packages=(
  build-essential pkg-config curl git
  libx11-6 libxcursor1 libxrandr2 libxi6 libxkbcommon0 libxkbcommon-x11-0
  libwayland-client0 libwayland-egl1 libgl1 libegl1
  fonts-noto-cjk
)
missing=()
for p in "${packages[@]}"; do
  dpkg -s "$p" >/dev/null 2>&1 || missing+=("$p")
done
if ((${#missing[@]})); then
  sudo apt-get update
  sudo apt-get install -y "${missing[@]}"
else
  echo "    すべて導入済み"
fi

echo "==> Rust"
if ! command -v cargo >/dev/null 2>&1 && [[ -x "$HOME/.cargo/bin/cargo" ]]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi
if ! command -v rustup >/dev/null 2>&1; then
  # apt の rustc は古いことが多く、依存クレートが要求するバージョンに届かないので rustup を使う。
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  export PATH="$HOME/.cargo/bin:$PATH"
fi
rustup toolchain install stable --profile minimal --component clippy rustfmt
rustup default stable
rustc --version

if ((build)); then
  echo "==> ビルドとテスト"
  cargo build --release
  cargo test
  echo
  echo "完了: ./target/release/vw で起動できます"
else
  echo
  echo "完了: cargo build --release でビルドできます"
fi
echo "新しい端末で cargo が見つからない場合は、source \"\$HOME/.cargo/env\" を実行してください"
