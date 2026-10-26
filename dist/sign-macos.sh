#!/bin/sh
# Signs a macOS executable with a Developer ID, with the hardened runtime
# and a secure timestamp, and has Apple notarize it, so that Gatekeeper
# lets it run when it was downloaded:
#
#   sh dist/sign-macos.sh <executable>
#
# It reads the certificate and the notarization key from the environment,
# as CI gives them from its secrets:
#
#   MACOS_CERTIFICATE           the Developer ID Application certificate
#                               with its private key, a .p12 in base64
#   MACOS_CERTIFICATE_PASSWORD  the .p12's password
#   APPLE_API_KEY               an App Store Connect API key, the .p8 in
#                               base64
#   APPLE_API_KEY_ID            its key ID
#   APPLE_API_ISSUER            its issuer ID
#
# Without a certificate it signs nothing, and says so. A notarized
# executable cannot be stapled; Gatekeeper asks Apple on its first run. A
# notarization Apple has not finished in 20 minutes is left to finish: the
# build is signed, and passes Gatekeeper once Apple is done.
set -eu
executable="$1"
say() { printf 'tondo sign: %s\n' "$*"; }
if [ -z "${MACOS_CERTIFICATE:-}" ]; then
    say "no MACOS_CERTIFICATE: $executable is not signed"
    exit 0
fi
for name in MACOS_CERTIFICATE_PASSWORD APPLE_API_KEY APPLE_API_KEY_ID APPLE_API_ISSUER; do
    eval "value=\${$name:-}"
    [ -n "$value" ] || {
        say "$name is missing: it is needed with MACOS_CERTIFICATE"
        exit 1
    }
done

work=$(mktemp -d)
keychain="$work/signing.keychain-db"
cleanup() {
    security delete-keychain "$keychain" 2>/dev/null || true
    rm -rf "$work"
}
trap cleanup EXIT

# A keychain of its own, which only this signing uses.
password=$(openssl rand -base64 24)
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 3600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
printf '%s' "$MACOS_CERTIFICATE" | base64 --decode >"$work/certificate.p12"
security import "$work/certificate.p12" -k "$keychain" -P "$MACOS_CERTIFICATE_PASSWORD" -T /usr/bin/codesign >/dev/null
security set-key-partition-list -S apple-tool:,apple: -s -k "$password" "$keychain" >/dev/null
# shellcheck disable=SC2046
security list-keychains -d user -s "$keychain" $(security list-keychains -d user | tr -d '"')
identity=$(security find-identity -v -p codesigning "$keychain" | awk '/Developer ID Application/ { print $2; exit }')
[ -n "$identity" ] || {
    say "the certificate is not a Developer ID Application one"
    exit 1
}

codesign --force --options runtime --timestamp --identifier io.github.tonyredondo.tondo \
    --sign "$identity" --keychain "$keychain" "$executable"
codesign --verify --strict --verbose=2 "$executable"
say "signed $executable"

# Apple notarizes a zip of it.
printf '%s' "$APPLE_API_KEY" | base64 --decode >"$work/key.p8"
ditto -c -k --keepParent "$executable" "$work/tondo.zip"
notary() { xcrun notarytool "$@" --key "$work/key.p8" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER"; }
notary submit "$work/tondo.zip" --output-format json >"$work/submitted.json" || {
    say "Apple did not take it to notarize: $(cat "$work/submitted.json")"
    exit 1
}
id=$(plutil -extract id raw -o - "$work/submitted.json")
say "Apple notarizes $executable as submission $id"
# Asked every 30 seconds, for 20 minutes at most.
deadline=$(($(date +%s) + 1200))
while :; do
    notary info "$id" --output-format json >"$work/info.json" 2>/dev/null || true
    status=$(plutil -extract status raw -o - "$work/info.json" 2>/dev/null || echo "")
    if [ "$status" != "In Progress" ] && [ -n "$status" ] || [ "$(date +%s)" -ge "$deadline" ]; then
        break
    fi
    sleep 30
done
case "$status" in
Accepted) ;;
"In Progress")
    # Gatekeeper looks the ticket up by the executable's hash, so this same
    # build passes once Apple is done.
    say "Apple is still notarizing $executable (submission $id): it is signed, and passes Gatekeeper once Apple is done"
    exit 0
    ;;
*)
    say "Apple did not notarize it: ${status:-no answer} $(cat "$work/info.json")"
    notary log "$id" || true
    exit 1
    ;;
esac
# Apple publishes the ticket of an accepted submission a little later, so
# Gatekeeper is asked for some minutes before the build goes on without.
deadline=$(($(date +%s) + 300))
until spctl --assess --type open --context context:primary-signature -v "$executable" >"$work/assessed" 2>&1 &&
    grep -q "Notarized Developer ID" "$work/assessed"; do
    if [ "$(date +%s)" -ge "$deadline" ]; then
        cat "$work/assessed"
        say "Apple notarized $executable, and Gatekeeper takes it for a notarized one once Apple publishes its ticket"
        exit 0
    fi
    sleep 15
done
cat "$work/assessed"
say "notarized $executable"
