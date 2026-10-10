#!/usr/bin/env bash
# 配布物に同梱するインストーラ。vw を ~/.local/bin に置き、アプリ一覧と「別のアプリで開く」に登録する。
#
#   ./install.sh            # インストール
#   ./install.sh --uninstall
#
# 管理者権限は使わない。システム全体（/usr/local）に入れたい場合は手で配置する。
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
bin_dir="$HOME/.local/bin"
app_dir="$HOME/.local/share/applications"

if [[ "${1:-}" == "--uninstall" ]]; then
  rm -f "$bin_dir/vw" "$app_dir/vw.desktop"
  command -v update-desktop-database >/dev/null && update-desktop-database "$app_dir" || true
  echo "vw を削除しました"
  exit 0
fi

mkdir -p "$bin_dir" "$app_dir"
install -m 755 "$here/vw" "$bin_dir/vw"
# Exec を絶対パスにする。~/.local/bin が PATH に入っていない環境でもアプリ一覧から起動できるように。
sed "s|^Exec=vw |Exec=$bin_dir/vw |" "$here/vw.desktop" > "$app_dir/vw.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$app_dir" || true

echo "インストールしました: $bin_dir/vw"
case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) echo "端末から vw で起動するには、~/.bashrc に export PATH=\"\$HOME/.local/bin:\$PATH\" を追加してください" ;;
esac
