# Общая версия FFmpeg для build-ffmpeg-min-*.sh — закреплена коммитом + sha256,
# один источник для Windows- и Android-сборки (не расходятся при обновлении).
FFMPEG_COMMIT=160737cf0da1915d8499881a8021d170f927411d
FFMPEG_SHA256=f44471d57f42892d97eed3064fe69848bd0fa0670960114a4fdac800c74a9df9

# Скачать и проверить sha256 архива по пінned commit/tag. Guard — по маркеру
# внутри распакованного каталога, а не по его существованию: прерванная
# распаковка (обрыв сети, Ctrl-C, диск переполнен) оставляет каталог без
# маркера, и следующий запуск перекачивает и проверяет заново вместо того,
# чтобы молча собрать из повреждённого дерева.
fetch_verified() {
  local url="$1" sha256="$2" archive="$3" dir="$4"
  if [ -f "$dir/.sticker-nah-verified" ]; then
    return 0
  fi
  rm -rf "$dir"
  curl -sL "$url" -o "$archive"
  echo "$sha256 *$archive" | sha256sum -c -
  tar xf "$archive"
  touch "$dir/.sticker-nah-verified"
}
