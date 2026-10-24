#!/bin/bash
pkill -x omuse >/dev/null 2>&1
for p in 8080 8081 8082 8090 8099 8091 8093 8094 8095 8096 8097 8098; do fuser -k $p/tcp >/dev/null 2>&1; done; sleep 1
pkill -x Xvfb >/dev/null 2>&1; rm -f /tmp/.X99-lock /tmp/.X11-unix/X99
