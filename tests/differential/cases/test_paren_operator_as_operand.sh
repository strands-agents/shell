v=-eq
[ \( -n "$v" \) ]; echo "paren-n-var: $?"
[ \( -n -eq \) ]; echo "paren-n-eq: $?"
[ \( -n -lt \) ]; echo "paren-n-lt: $?"
[ \( -z -eq \) ]; echo "paren-z-eq: $?"
[ ! -n -eq ]; echo "not-n-eq: $?"
[ -n -eq ]; echo "n-eq: $?"
[ -eq ]; echo "lone-eq: $?"
[ \( -eq \) ]; echo "paren-eq: $?"
[ \( abc -eq 1 \) ]; echo "paren-word-eq: $?"
[ \( 1 -eq 1 \) ]; echo "paren-int-eq: $?"
test \( -n -eq \); echo "test-paren-n-eq: $?"
v="a b"; if [ ! $v ]; then echo "not-two-words: taken"; else echo "not-two-words: not taken"; fi
v="a b c"; if [ ! $v ]; then echo "not-three-words: taken"; else echo "not-three-words: not taken"; fi
set -- x y; if [ ! "$@" ]; then echo "not-two-args: taken"; else echo "not-two-args: not taken"; fi
if [ \( a b \) ]; then echo "paren-two-words: taken"; else echo "paren-two-words: not taken"; fi
if [ ! x -a ]; then echo "not-dangling-and: taken"; else echo "not-dangling-and: not taken"; fi
if [ ! ! \( ! ]; then echo "not-unclosed-paren: taken"; else echo "not-unclosed-paren: not taken"; fi
