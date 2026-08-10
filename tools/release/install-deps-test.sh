#!/bin/sh
# Fixtures for the dependency step of install.sh.
#
#   install-deps-test.sh     runs every case, exits 1 on the first mismatch
#
# WHY THIS EXISTS
#
# install.sh installs the WebKit runtime itself rather than naming a package
# manager command and stopping. Before it runs anything it decides four
# things: whether this distribution has a package manager it knows, whether
# the missing soname has a package here, whether it is root or can become
# root, and whether --no-deps was passed. Each decision ends in a different
# sentence, and three of the four need a machine this one is not.
#
# VITRUM_SYSROOT is what puts them in reach: the installer reads the
# distribution and the installed libraries from under one directory, so a
# whole machine is an os-release file and an empty library directory. `id`,
# `sudo` and the package manager are stubs on PATH, so root and sudo are
# conditions this script sets rather than facts about whoever runs it.
#
# The two cases that matter most are `root, apt, package known` and `the
# package manager lies`. The first is a `curl | sh` on a fresh Ubuntu, which
# is the whole reason the step exists; the second holds down the rule that
# makes it trustworthy, which is that the library is looked for again
# afterwards. A step that treats an exit status of zero as proof installs
# nothing and reports success.
#
# Every case ends at the download, because a refusal that is not a refusal has
# to be seen going past the gate rather than merely not printing an error.
# `--base-url` names a directory that does not exist, so getting that far is
# `could not download` and nothing else.

set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
script=$here/../../install.sh
[ -f "$script" ] || { echo "install-deps-test: no install.sh two levels up" >&2; exit 2; }

work=$(mktemp -d) || exit 2
trap 'rm -rf "$work"' EXIT INT TERM

failures=0
cases=0

# The soname the installer refuses without, and the package Debian and Ubuntu
# carry it in. Read out of install.sh rather than typed here, so a rename in
# the table this test is about cannot leave the test passing against a name
# nothing uses.
soname=libwebkit2gtk-4.1.so.0
pkg=$(sed -n 's/.*libwebkit2gtk-4\.1\.so\.0) printf '\''\(libwebkit2gtk[^'\'']*\)'\''.*/\1/p' \
    "$script" | head -1)
[ -n "$pkg" ] || {
    echo "install-deps-test: install.sh no longer names a Debian package for $soname" >&2
    exit 2
}

# A machine, as a directory: what it calls itself and which libraries it has.
# `like` carries ID_LIKE, which is how a derivative is recognised.
machine() {
    m_root=$work/$1/root
    mkdir -p "$m_root/etc" "$m_root/usr/lib"
    {
        printf 'ID=%s\n' "$2"
        [ -z "${3:-}" ] || printf 'ID_LIKE=%s\n' "$3"
    } > "$m_root/etc/os-release"
    printf '%s' "$m_root"
}

# A PATH holding exactly the tools install.sh runs and nothing else, so `sudo`
# is absent when this says it is rather than when the machine happens to lack
# it. `uname` is stubbed too: the dependency step is the Linux branch, and a
# macOS host would otherwise run a different half of the script.
sandbox() {
    sb=$work/$1/bin
    mkdir -p "$sb"
    for tool in sh sed head tr grep cat cut mktemp rm rmdir mkdir dirname \
        readlink tar sha256sum shasum awk getconf chmod mv cp env; do
        real=$(command -v "$tool" 2>/dev/null) || continue
        case "$real" in
            /*) ln -sf "$real" "$sb/$tool" ;;
        esac
    done
    cat > "$sb/uname" <<'EOF'
#!/bin/sh
case "${1:-}" in
    -m) printf 'x86_64\n' ;;
    *) printf 'Linux\n' ;;
esac
EOF
    chmod +x "$sb/uname"
    printf '%s' "$sb"
}

# `id -u`. The installer reads it to decide whether it needs sudo at all.
stub_id() {
    cat > "$1/id" <<EOF
#!/bin/sh
printf '%s\n' $2
EOF
    chmod +x "$1/id"
}

stub_sudo() {
    cat > "$1/sudo" <<'EOF'
#!/bin/sh
exec "$@"
EOF
    chmod +x "$1/sudo"
}

# stub_apt <bin> <exit-status> <sysroot-or-empty>
#
# A package manager that exits with the status it was given and, when a
# sysroot is named, puts the library where the installer looks for it. Naming
# no sysroot is the package manager that exits zero and delivers nothing,
# which is the failure the second look exists to catch.
stub_apt() {
    cat > "$1/apt-get" <<EOF
#!/bin/sh
provide=$3
for a in "\$@"; do
    if [ "\$a" = install ] && [ -n "\$provide" ]; then
        mkdir -p "\$provide/usr/lib"
        : > "\$provide/usr/lib/$soname"
    fi
done
exit $2
EOF
    chmod +x "$1/apt-get"
}

# run <case> <expected-rc> -- <extra install.sh arguments>
#
# Leaves the combined output in $out and the status in $rc for the assertions
# below. HOME is inside the case directory, so nothing here reads or writes
# the home of whoever is running it.
run() {
    r_name=$1
    r_want=$2
    shift 2
    cases=$((cases + 1))
    r_home=$work/$r_case/home
    mkdir -p "$r_home"
    rc=0
    out=$(
        PATH="$sb" \
            HOME="$r_home" \
            XDG_DATA_HOME="$r_home/share" \
            VITRUM_SYSROOT="$root" \
            VITRUM_INSTALL_DIR="$r_home/bin" \
            VITRUM_NO_INTEGRATE=1 \
            https_proxy= HTTPS_PROXY= all_proxy= ALL_PROXY= \
            http_proxy= HTTP_PROXY= \
            sh "$script" --version=0.0.0 \
            --base-url="file://$work/no-such-mirror" "$@" 2>&1
    ) || rc=$?
    if [ "$rc" != "$r_want" ]; then
        fail "$r_name" "exit $r_want" "exit $rc"
    fi
}

fail() {
    printf 'install-deps-test: %s\n  expected %s\n  got      %s\n' "$1" "$2" "$3" >&2
    printf '%s\n' "$out" | sed 's/^/    | /' >&2
    failures=$((failures + 1))
}

# A sentence the run must have said.
says() {
    printf '%s\n' "$out" | grep -Fq -- "$2" || fail "$1" "the words '$2'" 'no such line'
}

# A whole line, matched end to end. The commands that acquire root are asserted
# this way on purpose: a substring cannot tell `sudo apt-get update` from
# `apt-get update`, and which of the two ran is the difference between a root
# machine and a machine that needed a password.
runs() {
    printf '%s\n' "$out" | grep -Fqx -- "$2" || fail "$1" "the line '$2'" 'no such line'
}

# Nothing the run may have said. A refusal that also ran the package manager is
# not a refusal.
silent_on() {
    if printf '%s\n' "$out" | grep -Fq -- "$2"; then
        fail "$1" "no mention of '$2'" 'it is mentioned'
    fi
}

# THE ONE THIS CHANGE EXISTS FOR. A fresh Ubuntu container: root, no webkit,
# apt, and no tty on stdin. It installs the package itself, without sudo
# because there is nothing to become, and gets past the gate.
r_case=root-apt
root=$(machine "$r_case" ubuntu debian)
sb=$(sandbox "$r_case")
stub_id "$sb" 0
stub_apt "$sb" 0 "$root"
run 'root, apt, package known' 1
runs 'root, apt, package known' "  apt-get update"
runs 'root, apt, package known' "  env DEBIAN_FRONTEND=noninteractive apt-get install -y $pkg"
says 'root, apt, package known' "$soname is installed."
says 'root, apt, package known' 'could not download'

# The same machine with a user on it. sudo is how the package gets installed,
# and it is printed with the command so the password prompt is not the first
# anyone hears of it.
r_case=user-sudo
root=$(machine "$r_case" ubuntu debian)
sb=$(sandbox "$r_case")
stub_id "$sb" 1000
stub_sudo "$sb"
stub_apt "$sb" 0 "$root"
run 'not root, sudo present' 1
runs 'not root, sudo present' "  sudo apt-get update"
runs 'not root, sudo present' "  sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y $pkg"
says 'not root, sudo present' 'could not download'

# No root and no way to get it. This is the refusal that has to stay a
# refusal, and it has to name the command rather than a package manager the
# reader is left to guess at.
r_case=user-no-sudo
root=$(machine "$r_case" ubuntu debian)
sb=$(sandbox "$r_case")
stub_id "$sb" 1000
stub_apt "$sb" 0 "$root"
run 'not root, no sudo' 1
says 'not root, no sudo' 'vitrum needs a WebKit runtime and this installer cannot install one'
says 'not root, no sudo' 'This is not root and there is no sudo on this machine'
says 'not root, no sudo' "sudo apt install $pkg"
silent_on 'not root, no sudo' 'apt-get'

# A distribution with no entry in the table. There is no package name to
# install and no package manager to install it with, so inventing a command
# would send someone to a package that does not exist.
r_case=unknown-distro
root=$(machine "$r_case" slackware)
sb=$(sandbox "$r_case")
stub_id "$sb" 0
stub_apt "$sb" 0 "$root"
run 'package unknown' 1
says 'package unknown' 'vitrum needs a WebKit runtime and this installer cannot install one'
says 'package unknown' 'No package on this distribution is known to provide it.'
silent_on 'package unknown' 'apt-get'

# --no-deps is the opt out, and the promise it makes is the message the
# installer used to end on: what is missing, and the one line that installs it.
r_case=no-deps
root=$(machine "$r_case" ubuntu debian)
sb=$(sandbox "$r_case")
stub_id "$sb" 0
stub_apt "$sb" 0 "$root"
run '--no-deps refuses with the manual command' 1 --no-deps
says '--no-deps refuses with the manual command' \
    'vitrum needs a WebKit runtime and this machine has none'
says '--no-deps refuses with the manual command' "sudo apt install $pkg"
silent_on '--no-deps refuses with the manual command' 'apt-get'

# A machine that already has it. Running a package manager here would ask for
# a password on a machine with nothing to install.
r_case=already-there
root=$(machine "$r_case" ubuntu debian)
: > "$root/usr/lib/$soname"
sb=$(sandbox "$r_case")
stub_id "$sb" 0
stub_apt "$sb" 0 "$root"
run 'the library is already installed' 1
silent_on 'the library is already installed' 'apt-get'
says 'the library is already installed' 'could not download'

# The package manager refuses. Its status is what the installer reports,
# because 100 and 1 mean different things to apt and neither means vitrum.
r_case=apt-fails
root=$(machine "$r_case" ubuntu debian)
sb=$(sandbox "$r_case")
stub_id "$sb" 0
stub_apt "$sb" 100 ''
run 'the package manager refuses' 1
says 'the package manager refuses' "the package manager could not install $pkg"
says 'the package manager refuses' 'It exited 100'
silent_on 'the package manager refuses' 'could not download'

# THE ONE THAT MUST NOT REGRESS. The package manager exits zero and the
# library is still not there, which is what a renamed package looks like. An
# installer that trusts the exit status installs a binary that opens no
# window and calls it done.
r_case=apt-lies
root=$(machine "$r_case" ubuntu debian)
sb=$(sandbox "$r_case")
stub_id "$sb" 0
stub_apt "$sb" 0 ''
run 'the package manager exits zero and delivers nothing' 1
says 'the package manager exits zero and delivers nothing' \
    "$pkg installed and $soname is still not here"
silent_on 'the package manager exits zero and delivers nothing' 'could not download'

if [ "$failures" -gt 0 ]; then
    printf 'install-deps-test: %s of %s cases failed\n' "$failures" "$cases" >&2
    exit 1
fi
printf 'install-deps-test: %s cases pass\n' "$cases"
