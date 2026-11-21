set -e
f() { false; echo in-f; }
if f; then echo then-f; fi
if { false; echo in-g; }; then echo then-g; fi
if ( false; echo in-s ); then echo then-s; fi
if eval 'false; echo in-e'; then echo then-e; fi
n=0
while f && [ "$n" -lt 1 ]; do n=$((n + 1)); done
echo after
