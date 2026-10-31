#!/bin/sh
# 紹介ページと使い方（site/）を、GitHub Pages に出す形に組み立てる。使い方は site/README.md。
set -eu

out=${1:-_site}
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

command -v pandoc >/dev/null 2>&1 || { echo "pandoc が見つかりません（Ubuntu: sudo apt install pandoc）" >&2; exit 1; }

rm -rf "$out"
mkdir -p "$out/images" "$out/manual" "$out/en/manual"
for item in site/*; do
  [ "$item" = site/manual ] || [ "$item" = site/README.md ] || [ "$item" = site/en ] || cp -r "$item" "$out/"
done
for item in site/en/*; do
  [ "$item" = site/en/manual ] || cp -r "$item" "$out/en/"
done
# スクリーンショットは README と同じ docs/ のものを使い回す（site/ に複製を置くと README と食い違うため）
cp docs/screenshot.png docs/images/*.png packaging/icons/vw.png "$out/images/"
# 英語版の使い方と紹介ページは、英語表示で撮った docs/images/en/ の画面を使う
mkdir -p "$out/images/en"
cp docs/images/en/*.png "$out/images/en/"

# 使い方は日本語（manual/）と英語（en/manual/）で同じ章立てにし、page で互いの同じ章へリンクする
for dir in manual en/manual; do
  for md in site/$dir/*.md; do
    name=$(basename "$md" .md)
    toc=--toc
    [ "$name" = index ] && toc=
    # gfm の見出しの id は GitHub と同じ付け方なので、日本語の見出しへのリンクもそのまま効く
    pandoc "$md" -f gfm+yaml_metadata_block -t html5 --template "site/$dir/template.html" \
      -V page="$name" $toc --toc-depth=2 -o "$out/$dir/$name.html"
  done
done

echo "組み立てました: $out"
