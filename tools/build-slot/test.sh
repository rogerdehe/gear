#!/bin/sh
# Checks build-slot.sh's rule with fixed readings: sh tools/build-slot/test.sh
here=$(cd "$(dirname "$0")" && pwd)
slot=$here/build-slot.sh
fail=0
show='echo "$BUILD_JOBS $CARGO_BUILD_JOBS $NEXTEST_TEST_THREADS $GOMAXPROCS"'

# expect <want "N N N GOMAXPROCS"> <load> <running> <free>
expect() {
    got=$(env -u BUILD_SLOT BUILD_SLOT_CORES=10 BUILD_SLOT_LOAD="$2" BUILD_SLOT_RUNNING="$3" BUILD_SLOT_FREE="$4" \
        "$slot" sh -c "$show" 2>/dev/null)
    if [ "$got" = "$1" ]; then echo "ok   load $2 running $3 free $4 -> $got"
    else echo "FAIL load $2 running $3 free $4: want $1, got $got"; fail=1; fi
}
expect "10 10 10 1" 0 1 60
expect "10 10 10 1"  3.2 1 60
expect "1 1 1 2"    0 8 60
expect "3 3 3 2"    0 3 60
expect "10 10 10 1"  12 1 60
expect "2 2 2 5"    0 1 20
expect "1 1 1 10"   0 1 10

# the command's exit status passes through
env -u BUILD_SLOT BUILD_SLOT_CORES=10 BUILD_SLOT_LOAD=0 BUILD_SLOT_RUNNING=1 BUILD_SLOT_FREE=60 "$slot" false 2>/dev/null
[ $? -eq 1 ] && echo "ok   exit status" || { echo "FAIL exit status"; fail=1; }

# a nested run leaves the outer run's numbers as they are
got=$(BUILD_SLOT=1 BUILD_JOBS=7 CARGO_BUILD_JOBS=7 NEXTEST_TEST_THREADS=7 GOMAXPROCS=2 "$slot" sh -c "$show" 2>&1)
[ "$got" = "7 7 7 2" ] && echo "ok   nested run passes through" || { echo "FAIL nested: $got"; fail=1; }

# one stderr line names the numbers
line=$(env -u BUILD_SLOT BUILD_SLOT_CORES=10 BUILD_SLOT_LOAD=0 BUILD_SLOT_RUNNING=2 BUILD_SLOT_FREE=60 "$slot" true 2>&1)
case $line in "build-slot: 5 (cores 10, load 0, running 2, free memory 60 %)") echo "ok   stderr line";;
    *) echo "FAIL stderr line: $line"; fail=1;; esac

exit $fail
