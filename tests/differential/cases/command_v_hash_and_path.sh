mkdir bin
printf '#!/bin/sh\necho mine\n' > bin/mytool
chmod +x bin/mytool
printf '#!/bin/sh\necho here\n' > hereonly
chmod +x hereonly
PATH="$PWD/bin:$PATH"
hash mytool
echo "hash status $?"
p=$(command -v mytool)
echo "found $? ${p#$PWD/}"
mv bin/mytool bin/other
p=$(command -v mytool)
echo "hashed after move $? ${p#$PWD/}"
hash -r
p=$(command -v mytool)
echo "after hash -r $? [${p#$PWD/}]"
(PATH=":/usr/bin:/bin"; command -v hereonly; echo "leading empty entry $?")
(PATH="/usr/bin::/bin"; command -v hereonly; echo "middle empty entry $?")
(PATH="/usr/bin:/bin:"; command -v hereonly; echo "trailing empty entry $?")
(PATH="/usr/bin:/bin"; command -v hereonly; echo "no empty entry $?")
(PATH=":/usr/bin:/bin"; hereonly; echo "ran through empty entry $?")
(unset PATH; command -v lash; echo "unset PATH, not a command $?")
(unset PATH; command -v other; echo "unset PATH, not in cwd $?")
