# known-difference: command -v does not look up a name containing a slash; fixed by #272
mkdir bin
printf '#!/bin/sh\necho here\n' > here
cp here bin/tool
chmod +x here bin/tool
command -v ./here; echo "status $?"
command -v bin/tool; echo "status $?"
command -v ./missing; echo "status $?"
