#!/usr/bin/env bash
# Signs pingtable.exe and pingtable-cli.exe in dist/windows with Microsoft
# Artifact Signing (formerly Trusted Signing), so Windows shows the publisher's
# name instead of "unknown publisher". Uses jsign, which runs anywhere Java does.
#
#   scripts/sign-windows.sh               sign the two programs, then check them
#   scripts/sign-windows.sh --configured  exit 0 when signing is set up, 3 when not
#   scripts/sign-windows.sh --verify      only check the signatures (needs Windows)
#
# scripts/package-windows.sh runs this by itself once signing is set up.
# What has to exist first, and how to get it: docs/signing.md.
#
# Settings come from the environment, or from a file named signing.env beside
# Cargo.toml (KEY=value lines; it is not committed). None of them is a secret:
#   PINGTABLE_SIGN_ENDPOINT  the account's region, for example eus.codesigning.azure.net
#   PINGTABLE_SIGN_ACCOUNT   the Artifact Signing account name
#   PINGTABLE_SIGN_PROFILE   the certificate profile name
#   JSIGN_JAR                path to jsign-<version>.jar, unless `jsign` is on the PATH
#   JAVA                     the java to run it with, unless `java` is on the PATH
#
# The access token is never stored. It comes from PINGTABLE_SIGN_TOKEN if set,
# else from `az account get-access-token` (run `az login` first), else jsign
# reads AZURE_TENANT_ID, AZURE_CLIENT_ID and AZURE_CLIENT_SECRET itself.
set -euo pipefail
cd "$(dirname "$0")/.."

out=dist/windows
programs=("$out/pingtable.exe" "$out/pingtable-cli.exe")
marker="$out/SIGNED.txt"

die() { echo "sign-windows: $*" >&2; exit 1; }

# Values from signing.env fill in only what the environment does not set.
if [ -f signing.env ]; then
    while IFS='=' read -r key value; do
        case "$key" in
            PINGTABLE_SIGN_ENDPOINT | PINGTABLE_SIGN_ACCOUNT | PINGTABLE_SIGN_PROFILE | JSIGN_JAR | JAVA)
                value=${value%$'\r'}
                [ -n "${!key:-}" ] || export "$key=$value"
                ;;
        esac
    done < <(grep -v '^[[:space:]]*#' signing.env || true)
fi

configured() {
    [ -n "${PINGTABLE_SIGN_ENDPOINT:-}" ] && [ -n "${PINGTABLE_SIGN_ACCOUNT:-}" ] && [ -n "${PINGTABLE_SIGN_PROFILE:-}" ]
}

# Ask Windows what it makes of each signature. Prints "Status|Signer" a file.
verify() {
    command -v powershell.exe >/dev/null 2>&1 && command -v wslpath >/dev/null 2>&1 || {
        echo "sign-windows: signatures not checked (that needs Windows; on Windows: Get-AuthenticodeSignature FILE)"
        return 0
    }
    local program answer status signer bad=0
    for program in "${programs[@]}"; do
        test -f "$program" || die "$program is missing; run scripts/build-windows.sh"
        answer=$(cd /mnt/c && powershell.exe -NoProfile -Command "\$s = Get-AuthenticodeSignature -LiteralPath '$(wslpath -w "$PWD_AT_START/$program")'; \$s.Status.ToString() + '|' + \$s.SignerCertificate.Subject" | tr -d '\r')
        status=${answer%%|*}
        signer=${answer#*|}
        if [ "$status" = Valid ]; then
            echo "  ok        $program  signed by ${signer:-unknown}"
        else
            echo "  $status  $program"
            bad=1
        fi
    done
    return "$bad"
}
PWD_AT_START=$PWD

case "${1:-}" in
    --configured)
        configured && exit 0
        exit 3
        ;;
    --verify)
        verify || die "not every program carries a valid signature"
        exit 0
        ;;
    "") ;;
    *) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac

configured || die "signing is not set up. Set PINGTABLE_SIGN_ENDPOINT, PINGTABLE_SIGN_ACCOUNT and PINGTABLE_SIGN_PROFILE (see docs/signing.md)."
for program in "${programs[@]}"; do
    test -f "$program" || die "$program is missing; run scripts/build-windows.sh"
done

if [ -n "${JSIGN_JAR:-}" ]; then
    test -f "$JSIGN_JAR" || die "JSIGN_JAR points at $JSIGN_JAR, which is not there"
    java=${JAVA:-java}
    command -v "$java" >/dev/null 2>&1 || die "java was not found; set JAVA to the java program"
    jsign=("$java" -jar "$JSIGN_JAR")
elif command -v jsign >/dev/null 2>&1; then
    jsign=(jsign)
else
    die "jsign was not found. Download jsign-<version>.jar and set JSIGN_JAR to it (see docs/signing.md)."
fi

options=(--storetype TRUSTEDSIGNING
    --keystore "$PINGTABLE_SIGN_ENDPOINT"
    --alias "$PINGTABLE_SIGN_ACCOUNT/$PINGTABLE_SIGN_PROFILE"
    --name pingtable
    --url https://github.com/lorincz-llc/pingtable)

token=${PINGTABLE_SIGN_TOKEN:-}
if [ -z "$token" ] && [ -z "${AZURE_CLIENT_SECRET:-}" ]; then
    command -v az >/dev/null 2>&1 || die "no access token: install the Azure CLI and run 'az login', or set PINGTABLE_SIGN_TOKEN"
    token=$(az account get-access-token --resource https://codesigning.azure.net --query accessToken --output tsv | tr -d '\r') \
        || die "the Azure CLI gave no token. Run: az login"
    test -n "$token" || die "the Azure CLI gave an empty token. Run: az login"
fi
if [ -n "$token" ]; then
    options+=(--storepass "$token")
fi

# One run for both files; the service timestamps them itself.
rm -f "$marker"
"${jsign[@]}" "${options[@]}" "${programs[@]}"

echo "sign-windows: jsign finished for ${#programs[@]} programs"
verify || die "Windows does not accept the new signatures; see above"

# The note packaging looks for. The hashes tie it to these exact files.
{
    echo "Signed with Microsoft Artifact Signing, account $PINGTABLE_SIGN_ACCOUNT, profile $PINGTABLE_SIGN_PROFILE."
    sha256sum "${programs[@]}"
} > "$marker"
