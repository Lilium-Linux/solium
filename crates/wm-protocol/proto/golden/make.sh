#!/bin/sh
# Encode each message here with protoc, protobuf's own implementation, so
# the crate's tests can hold its encoding to it: what it reads from these
# bytes is the message, and what it writes for the message is these bytes.
#
# Run after the protocol changes, with any protoc of version 3.15 or later:
#     PROTOC=/path/to/protoc crates/wm-protocol/proto/golden/make.sh
set -eu
cd "$(dirname "$0")"
protoc=${PROTOC:-protoc}
for text in *.txt; do
    "$protoc" --proto_path=.. --encode=solium.wm.Message solium_wm.proto \
        < "$text" > "${text%.txt}.bin"
done
