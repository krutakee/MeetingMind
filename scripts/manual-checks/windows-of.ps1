# Lists the top-level windows of a process (visible or not), to see dialogs and the control window.
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class WindowList {
    static WindowList() { SetProcessDPIAware(); } // report physical pixels, like the recording
    [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
    delegate bool EnumProc(IntPtr hwnd, IntPtr lparam);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc proc, IntPtr lparam);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int max);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, StringBuilder text, int max);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] static extern uint GetWindowDisplayAffinity(IntPtr hwnd, out uint affinity);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    public static List<string> Of(uint wantedPid) {
        var found = new List<string>();
        EnumWindows((hwnd, l) => {
            uint pid; GetWindowThreadProcessId(hwnd, out pid);
            if (pid != wantedPid) return true;
            var title = new StringBuilder(256); GetWindowText(hwnd, title, 256);
            var cls = new StringBuilder(256); GetClassName(hwnd, cls, 256);
            RECT r; GetWindowRect(hwnd, out r);
            uint affinity = 0; GetWindowDisplayAffinity(hwnd, out affinity);
            found.Add(string.Format("visible={0} class={1} title='{2}' size={3}x{4} at=({5},{6}) affinity=0x{7:X}", IsWindowVisible(hwnd), cls, title, r.Right - r.Left, r.Bottom - r.Top, r.Left, r.Top, affinity));
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
'@
