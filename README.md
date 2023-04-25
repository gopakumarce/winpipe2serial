# winpipe2serial

Windows pipe to serial port, for hyper-V VMs to access windows host COM ports,
for use with say an Arduino/esp32 thats plugged into the windows COM port and 
arduino/esp32 programming being done inside a hyper-V ubuntu VM

To create a COM port on hyper-V ubuntu VM and map it to a windows pipe, open a 
powershell in admin mode and say 

Set-VMComPort <VM name> -Number <1 or 2, 1 for COM1, 2 for COM2> -Path \\.\pipe\myserialpipe

We can confirm the setting by saying "Get-VMComPort <VM name>"

Now shut down the VM and reboot it and you can see the new COM ports in the hyper-V settings 
for that VM and also see a /dev/ttyS0 for COM1 and /dev/ttyS1 for COM2 inside the ubuntu VM.
We can use getserial application to confirm what serial ports are actually in use, and
screen on linux to read from the serial port. Screen can get messy if we leave multiple 
screens opened on the same ttyS in which case we wont know which screen the output goes to.
I personally use minicom, I know there is one minicom running where I can see the output and
send data back

After the VM is setup with a COM port mapped to a windows pipe, on the windows side we run the 
winpipe2serial binary to do pipe to host COM serial and vice versa

To build this binary, install rust (rustup-init.exe is the easiest way to install on windows,
it will automaticall download the required Visual Studio C++ tools etc..) and after installing
rust open a new cmd window and say "cargo build". Then open a cmd window as administrator and
say "target\debug\winpipe2serial.exe COM3 myserialpipe" where COM3 is the example COM port 
specified in Set-VMComPort and myserialpipe is the pipe specified in the same command.