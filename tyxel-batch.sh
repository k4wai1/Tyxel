#!/data/data/com.termux/files/usr/bin/bash
# tyxel-batch.sh — convierte TODOS los videos de ~/downloads al pipeline Tyxel:
#   1) .txv densidad UHD (180 columnas) -> render MP4 1080p + audio -> galería
#   2) .txv copia al CUARTO de la resolución de celdas (90 columnas = 1/4 del
#      área, 1/2 lineal) -> render MP4 720p + audio -> galería
# Idempotente: si el .txv ya existe, lo salta. Re-ejecutable como un comando:
#   ~/Tyxel/tyxel-batch.sh
set -u
cd "$(dirname "$0")"
ENC=./target/release/tyxel-enc
PLAY=./target/release/tyxel-play
SRC="${1:-$HOME/downloads}"
RENDERS="$PWD/Renders"
GAL=/sdcard/Movies
mkdir -p "$RENDERS" "$GAL"
fail=0

for f in "$SRC"/*.mp4 "$SRC"/*.webm "$SRC"/*.mkv "$SRC"/*.avi "$SRC"/*.mov; do
	[ -e "$f" ] || continue
	b="$(basename "$f")"; b="${b%.*}"; b="${b// /_}"

	# ---------- PASE 1: densidad UHD (180 cols) -> 1080p ----------
	if [ ! -e "$RENDERS/${b}_UHD180w.txv" ]; then
		echo "== [UHD180] $b =="
		"$ENC" video --video "$f" --width 180 --fps 30 \
			-o "$RENDERS/${b}_UHD180w.txv" || { fail=1; echo "ENC FAIL $b"; continue; }
	fi
	if [ ! -e "$RENDERS/${b}_UHD180w.mp4" ]; then
		"$PLAY" render "$RENDERS/${b}_UHD180w.txv" \
			-o "$RENDERS/${b}_UHD180w.mp4" --res 1080p --fps 30 \
			--audio-from "$f" || { fail=1; echo "RENDER FAIL $b"; continue; }
		cp "$RENDERS/${b}_UHD180w.mp4" \
			"$GAL/Tyxel_1080p_UHD180w_${b}.mp4"
	fi

	# ---------- PASE 2: copia al CUARTO (90 cols = 1/4 celdas) ----------
	if [ ! -e "$RENDERS/${b}_SD90w.txv" ]; then
		echo "== [SD90 (1/4)] $b =="
		"$ENC" video --video "$f" --width 90 --fps 30 \
			-o "$RENDERS/${b}_SD90w.txv" || { fail=1; echo "ENC FAIL $b (SD)"; continue; }
	fi
	if [ ! -e "$RENDERS/${b}_SD90w.mp4" ]; then
		"$PLAY" render "$RENDERS/${b}_SD90w.txv" \
			-o "$RENDERS/${b}_SD90w.mp4" --res 720p --fps 30 \
			--audio-from "$f" || { fail=1; echo "RENDER FAIL $b (SD)"; continue; }
		cp "$RENDERS/${b}_SD90w.mp4" \
			"$GAL/Tyxel_720p_SD90w_${b}.mp4"
	fi
done
echo "== tyxel-batch terminado (fail=$fail) =="
exit $fail
