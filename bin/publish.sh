#!/usr/bin/env bash
set -e
USAGE="Publish a new release of a cynthia crate

USAGE:
    $(basename "$0") [OPTIONS] [CRATE] [VERSION]

OPTIONS:
    -v, --verbose   Use verbose Cargo output
    -d, --dry-run   Perform a dry run (do not publish or tag the release)
    -h, --help      Show this help text and exit"

DRY_RUN=""
VERBOSE=""

err() {
    echo -e "\e[31m\e[1merror:\e[0m $@" 1>&2;
}

status() {
    WIDTH=12
    printf "\e[32m\e[1m%${WIDTH}s\e[0m %s\n" "$1" "$2"
}

verify() {
    status "Verifying" "if $CRATE v$VERSION can be released"
    ACTUAL=$(cargo pkgid | sed -n 's/.*#\(.*\)/\1/p')

    if [ "$ACTUAL" != "$VERSION" ]; then
        err "expected to release version $VERSION, but Cargo.toml contained $ACTUAL"
        exit 1
    fi

    if git tag -l | grep -Fxq "$TAG" ; then
        err "git tag \`$TAG\` already exists"
        exit 1
    fi

    PATH_DEPS=$(grep -F "path = \"" Cargo.toml | sed -e 's/^/  /')
    if [ -n "$PATH_DEPS" ]; then
        err "crate \`$CRATE\` contained path dependencies:\n$PATH_DEPS"
        echo "path dependencies must be removed prior to release"
        exit 1
    fi
}

release() {
    status "Releasing" "$CRATE v$VERSION"
    cargo package $VERBOSE
    cargo publish $VERBOSE $DRY_RUN

    status "Tagging" "$TAG"
    if [ -n "$DRY_RUN" ]; then
        echo "# git tag $TAG && git push --tags"
    else
        git tag "$TAG" && git push --tags
    fi
}

while [[ $# -gt 0 ]]
do

case "$1" in
    -h|--help)
    echo "$USAGE"
    exit 0
    ;;
    -v|--verbose)
    VERBOSE="--verbose"
    set +x
    shift
    ;;
    -d|--dry-run)
    DRY_RUN="--dry-run"
    shift
    ;;
    -*)
    err "unknown flag \"$1\""
    echo "$USAGE"
    exit 1
    ;;
    *)
    if [ -z "$CRATE" ]; then
        CRATE="$1"
    elif [ -z "$VERSION" ]; then
        VERSION="$1"
    else
        err "unknown positional argument \"$1\""
        echo "$USAGE"
        exit 1
    fi
    shift
    ;;
esac
done

if [ -z "$VERSION" ]; then
    err "no version specified!"
    HELP=1
fi

if [ -n "$CRATE" ]; then
    TAG="$CRATE-$VERSION"
else
    err "no crate specified!"
    HELP=1
fi

if [ -n "$HELP" ]; then
    echo "$USAGE"
    exit 1
fi

if [ -d "$CRATE" ]; then
    (cd "$CRATE" && verify && release )
else
    err "no such crate \"$CRATE\""
    exit 1
fi
