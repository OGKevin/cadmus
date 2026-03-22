#! /bin/sh

# When this script is called in a diff dir other than the cadmus installation
# we need to change to the installation dir.
#
# For example, when spawned via NickelMenu, the currnt workding dir is whatever
# Nickel uses.
WORKDIR=$(dirname "$0")
cd "$WORKDIR" || exit 1

CADMUS_SET_FRAMEBUFFER_DEPTH=1
CADMUS_CONVERT_DICTIONARIES=1

# shellcheck disable=SC1091
[ -e config.sh ] && . config.sh

# shellcheck disable=SC2046
export $(grep -sE '^(INTERFACE|WIFI_MODULE|DBUS_SESSION_BUS_ADDRESS|NICKEL_HOME|LANG)=' /proc/"$(pidof -s nickel)"/environ)
sync
killall -TERM nickel hindenburg sickel fickel adobehost foxitpdf iink dhcpcd-dbus dhcpcd fmon >/dev/null 2>&1

if [ -e /sys/class/leds/LED ]; then
  LEDS_INTERFACE=/sys/class/leds/LED/brightness
  STANDARD_LEDS=1
elif [ -e /sys/class/leds/GLED ]; then
  LEDS_INTERFACE=/sys/class/leds/GLED/brightness
  STANDARD_LEDS=1
elif [ -e /sys/class/leds/bd71828-green-led ]; then
  LEDS_INTERFACE=/sys/class/leds/bd71828-green-led/brightness
  STANDARD_LEDS=1
elif [ -e /sys/devices/platform/ntx_led/lit ]; then
  LEDS_INTERFACE=/sys/devices/platform/ntx_led/lit
  STANDARD_LEDS=0
elif [ -e /sys/devices/platform/pmic_light.1/lit ]; then
  LEDS_INTERFACE=/sys/devices/platform/pmic_light.1/lit
  STANDARD_LEDS=0
fi

# Turn off the LEDs
if [ "$STANDARD_LEDS" -eq 1 ]; then
  echo 0 >"$LEDS_INTERFACE"
else
  # https://www.tablix.org/~avian/blog/archives/2013/03/blinken_kindle/
  for ch in 3 4 5; do
    echo "ch ${ch}" >"$LEDS_INTERFACE"
    echo "cur 1" >"$LEDS_INTERFACE"
    echo "dc 0" >"$LEDS_INTERFACE"
  done
fi

# Remount the SD card read-write if it's mounted read-only
grep -q ' /mnt/sd .*[ ,]ro[ ,]' /proc/mounts && mount -o remount,rw /mnt/sd

# Define model number used for device detection
KOBO_TAG=/mnt/onboard/.kobo/version
if [ -e "$KOBO_TAG" ]; then
  MODEL_NUMBER=$(cut -f 6 -d ',' "$KOBO_TAG" | sed -e 's/^[0-]*//')

  export MODEL_NUMBER
fi

export LD_LIBRARY_PATH="libs:${LD_LIBRARY_PATH}"

[ -e info.log ] && [ "$(stat -c '%s' info.log)" -gt $((1 << 18)) ] && mv info.log archive.log

[ "$CADMUS_CONVERT_DICTIONARIES" ] && find -L dictionaries -name '*.ifo' -exec ./convert-dictionary.sh {} \;

if [ "$CADMUS_SET_FRAMEBUFFER_DEPTH" ]; then
  case "${PRODUCT}:${MODEL_NUMBER}" in
    kraken:* | pixie:* | dragon:* | phoenix:* | dahlia:* | alyssum:* | pika:* | daylight:* | star:375 | snow:374)
      ORIG_BPP=$(./bin/utils/fbdepth -g)
      ;;
    *)
      unset ORIG_BPP
      ;;
  esac
fi

[ "$ORIG_BPP" ] && ./bin/utils/fbdepth -q -d 8

CRASH_COUNT=0

exit_cadmus() {
  [ "$ORIG_BPP" ] && ./bin/utils/fbdepth -q -d "$ORIG_BPP"

  if [ -e /tmp/reboot ]; then
    reboot
  elif [ -e /tmp/power_off ]; then
    poweroff -f
  else
    ./nickel.sh &
  fi

  exit
}

while true; do
  LIBC_FATAL_STDERR_=1 ./cadmus >>info.log 2>&1
  EXIT_CODE=$?

  if [ -f /tmp/restart ]; then
    rm /tmp/restart
    CRASH_COUNT=0
    cd "$WORKDIR" || exit_cadmus
    continue
  fi

  if [ $EXIT_CODE -eq 0 ]; then
    break
  fi

  CRASH_COUNT=$((CRASH_COUNT + 1))

  if [ $CRASH_COUNT -ge 3 ]; then
    echo "cadmus.sh: CRASH_COUNT -ge 3, not restarting." >>info.log
    break
  fi

  cd "$WORKDIR" || exit_cadmus
done

exit_cadmus
