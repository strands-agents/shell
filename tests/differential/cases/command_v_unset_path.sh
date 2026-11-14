# known-difference: with PATH unset, bash still finds an executable in the current directory and not ls; strands-shell finds no file but still reports its registered commands
printf '#!/bin/sh\necho here\n' > hereonly
chmod +x hereonly
(unset PATH; p=$(command -v hereonly); echo "cwd executable $? ${p#$PWD/}")
(unset PATH; command -v ls; echo "ls $?")
