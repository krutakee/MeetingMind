# Sends key combinations to the desktop, to test global shortcuts. Dot-source this file.
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Keys {
    [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    const uint KEYUP = 2;
    public static void Chord(byte[] modifiers, byte key) {
        foreach (var m in modifiers) keybd_event(m, 0, 0, UIntPtr.Zero);
        keybd_event(key, 0, 0, UIntPtr.Zero);
        System.Threading.Thread.Sleep(60);
        keybd_event(key, 0, KEYUP, UIntPtr.Zero);
        for (int i = modifiers.Length - 1; i >= 0; i--) keybd_event(modifiers[i], 0, KEYUP, UIntPtr.Zero);
    }
}
'@
$VK_CONTROL = 0x11; $VK_SHIFT = 0x10; $VK_S = 0x53; $VK_P = 0x50; $VK_RETURN = 0x0D
function Press-CtrlShift([byte]$key) { [Keys]::Chord([byte[]]@($VK_CONTROL, $VK_SHIFT), $key) }
function Press-Enter { [Keys]::Chord([byte[]]@(), $VK_RETURN) }
