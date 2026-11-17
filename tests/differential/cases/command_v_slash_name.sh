mkdir bin
printf '#!/bin/sh\necho here\n' > here
cp here bin/tool
chmod +x here bin/tool
command -v ./here; echo "status $?"
command -v bin/tool; echo "status $?"
command -v ./missing; echo "status $?"
