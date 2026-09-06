# Does this Qt adopt a foreign EGL context?

The whole GPU path for in-compositor QML turns on one call. If Qt can adopt the
compositor's EGL context, it can render into a texture we sample directly. If
it cannot, Qt renders on a context of its own and the only way across is a
shared buffer.

    podman run --rm -v $HOME:$HOME -w $PWD localhost/solium-build:fc44 \
        sh -c 'g++ -fPIC probe.cpp -o probe $(pkg-config --cflags --libs Qt6Gui) -lEGL'
    QT_QPA_PLATFORM=offscreen ./probe
    QT_QPA_PLATFORM=eglfs ./probe

This exists because the answer has now been derived twice, and it is a
property of the installed Qt rather than of anything in this repository — so it
is worth re-running after a Qt update rather than reasoning about.
