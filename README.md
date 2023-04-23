# winpipe2serial

Windows pipe to serial port, for hyper-V ubuntu to access windows COM ports,
for use with say an Arduino/esp32 thats plugged into the windows COM port

To create a COM port on hyper-V ubuntu mapped to a windows pipe, say 

Set-VMComPort <VM name> -Number <1 or 2, 1 for COM1, 2 for COM2> -Path \\.\pipe\myserialpipe

We can confirm the setting by saying "Get-VMComPort <VM name>"

Now shut down the VM and reboot it and you can see a /dev/ttyS0 for COM1 and /dev/ttyS1 for COM2
We can use getserial application to confirm what serial ports are actually in use, and
screen on linux to read from the serial port

Now on the windows side we run the winpipe2serial binary to do a pipe to serial and vice versa
conversion

NOTE: The binary has to be run in admin mode to be able to read/write from the COM ports