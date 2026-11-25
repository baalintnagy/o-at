#!/bin/sh
# Installs or uninstalls O-AT from the unpacked release folder.
#
#   ./install.sh                 plugins, applications and menu entries for this user
#   ./install.sh --system        the same for every user (run with sudo)
#   ./install.sh --uninstall     removes what an installation added (add --system for
#                                a system-wide installation)
#   ./install.sh --skip-plugins  only the applications; --skip-apps only the plugins
#
# The applications' settings in ~/.config/O-AUD-RX and ~/.config/O-AUD-TX stay.
set -eu

source_dir=$(cd "$(dirname "$0")" && pwd)
system=false
uninstall=false
plugins=true
apps=true
for argument in "$@"; do
    case "$argument" in
        --system) system=true ;;
        --uninstall) uninstall=true ;;
        --skip-plugins) plugins=false ;;
        --skip-apps) apps=false ;;
        -h | --help) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "Unknown option: $argument (see --help)" >&2; exit 2 ;;
    esac
done

if $system; then
    if [ "$(id -u)" -ne 0 ]; then
        echo "--system needs root: sudo ./install.sh --system" >&2
        exit 1
    fi
    vst3_dir=/usr/lib/vst3
    clap_dir=/usr/lib/clap
    app_dir=/opt/o-at
    bin_dir=/usr/local/bin
    share_dir=/usr/local/share
else
    vst3_dir=$HOME/.vst3
    clap_dir=$HOME/.clap
    app_dir=$HOME/.local/lib/o-at
    bin_dir=$HOME/.local/bin
    share_dir=${XDG_DATA_HOME:-$HOME/.local/share}
fi
desktop_dir=$share_dir/applications
icon_dir=$share_dir/icons/hicolor/256x256/apps

plugin_names="O-NEC O-NR O-AUD-TX O-AUD-RX O-AUD-A"
# Application: file, command, icon, menu name, description.
app_table="O-AUD-RX|o-aud-rx|o-aud-rx|O-AUD-RX|Plays an O-AT audio bus on an output device
O-AUD-TX|o-aud-tx|o-aud-tx|O-AUD-TX|Sends input devices to O-AT audio buses
O-AUD-A-standalone|o-aud-a|o-aud-a|O-AUD-A|Analyzes an audio input device
O-NEC-standalone|o-nec|o-nec|O-NEC|Voice cleanup without a plugin host
O-NR-standalone|o-nr|o-nr|O-NR|Noise reduction without a plugin host"

if ! $uninstall; then
    for name in $plugin_names; do
        [ -d "$source_dir/VST3/$name.vst3" ] && [ -f "$source_dir/CLAP/$name.clap" ] || {
            echo "Run this script from the unpacked O-AT folder ($name plugins are missing)." >&2
            exit 1
        }
    done
fi

if $plugins; then
    for name in $plugin_names; do
        rm -rf "${vst3_dir:?}/$name.vst3"
        if $uninstall; then
            rm -f "$clap_dir/$name.clap"
            echo "Removed $name plugins"
        else
            mkdir -p "$vst3_dir" "$clap_dir"
            cp -R "$source_dir/VST3/$name.vst3" "$vst3_dir/"
            cp "$source_dir/CLAP/$name.clap" "$clap_dir/"
            echo "Installed $name.vst3 and $name.clap"
        fi
    done
fi

if $apps; then
    if ! $uninstall; then
        mkdir -p "$app_dir" "$bin_dir" "$desktop_dir" "$icon_dir"
    fi
    echo "$app_table" | while IFS='|' read -r file command icon title description; do
        desktop="$desktop_dir/o-at-$command.desktop"
        if $uninstall; then
            rm -f "$app_dir/$file" "$bin_dir/$command" "$desktop" "$icon_dir/o-at-$icon.png"
            continue
        fi
        install -m 755 "$source_dir/$file" "$app_dir/$file"
        ln -sf "$app_dir/$file" "$bin_dir/$command"
        if [ -f "$source_dir/icons/$icon.png" ]; then
            install -m 644 "$source_dir/icons/$icon.png" "$icon_dir/o-at-$icon.png"
        fi
        cat > "$desktop" <<EOF
[Desktop Entry]
Type=Application
Name=$title
Comment=$description
Exec="$app_dir/$file"
Icon=o-at-$icon
Terminal=false
Categories=AudioVideo;Audio;
EOF
        echo "Installed $title as $bin_dir/$command"
    done
    if $uninstall; then
        rmdir "$app_dir" 2>/dev/null || true
        echo "Removed the applications and their menu entries"
    fi
    command -v update-desktop-database > /dev/null 2>&1 &&
        update-desktop-database "$desktop_dir" > /dev/null 2>&1 || true
fi

if $uninstall; then
    echo "O-AT is uninstalled. Settings in ~/.config/O-AUD-RX and ~/.config/O-AUD-TX stay."
else
    echo "O-AT is installed. Rescan plugins in your host; they are listed under the vendor \"o-at\"."
    if $apps; then
        case ":$PATH:" in
            *":$bin_dir:"*) ;;
            *) echo "Add $bin_dir to PATH to start the applications by name." ;;
        esac
    fi
fi
