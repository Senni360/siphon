#requires -Version 5.1
param([Parameter(Mandatory = $true)][string]$OutputPath)
$ErrorActionPreference = 'Stop'
# Compile with Windows PowerShell's .NET Framework so the fixture also runs
# outside the PowerShell 7 process. This fixture does not use Python or network.
Add-Type -OutputAssembly $OutputPath -OutputType ConsoleApplication -TypeDefinition @'
using System;
using System.IO;
using System.Text;
using System.Reflection;
public class InstallerStub {
    public static void Main(string[] args) {
        if (args.Length > 0 && args[0] == "-c") return;
        if (args.Length > 0 && args[0] != "-u") { Console.WriteLine("siphon-host 0.2.0 (protocol 1)"); return; }
        if (Path.GetFileName(Assembly.GetExecutingAssembly().Location) == "siphon-tray.exe") {
            File.WriteAllText(Path.Combine(Environment.GetEnvironmentVariable("LOCALAPPDATA"), "tray-started"), "unexpected");
            return;
        }
        var input = Console.OpenStandardInput();
        var output = Console.OpenStandardOutput();
        var header = new byte[4];
        Read(input, header);
        var body = new byte[BitConverter.ToUInt32(header, 0)];
        Read(input, body);
        var request = Encoding.UTF8.GetString(body);
        string reply;
        if (request.Contains("install-ytdlp")) {
            var directory = Path.Combine(Environment.GetEnvironmentVariable("LOCALAPPDATA"), "Siphon", "bin", "yt-dlp");
            Directory.CreateDirectory(directory);
            File.Copy(Assembly.GetExecutingAssembly().Location, Path.Combine(directory, "yt-dlp.exe"), true);
            reply = "{\"id\":\"install-ytdlp\",\"type\":\"result\",\"ok\":true,\"data\":{\"applied\":true}}";
        } else {
            reply = "{\"id\":\"t\",\"type\":\"result\",\"ok\":true,\"data\":{\"hostVersion\":\"stub\",\"ytdlpVersion\":\"test\",\"ffmpeg\":{\"found\":true,\"version\":\"test\",\"path\":\"test\"},\"jsRuntime\":null}}";
            if (Environment.GetEnvironmentVariable("SIPHON_INSTALLER_TEST_FAIL") == "1") reply = reply.Replace("\"found\":true", "\"found\":false");
        }
        var bytes = Encoding.UTF8.GetBytes(reply);
        output.Write(BitConverter.GetBytes(bytes.Length), 0, 4);
        output.Write(bytes, 0, bytes.Length);
        output.Flush();
    }
    static void Read(Stream input, byte[] buffer) {
        int offset = 0;
        while (offset < buffer.Length) {
            int count = input.Read(buffer, offset, buffer.Length - offset);
            if (count == 0) throw new EndOfStreamException();
            offset += count;
        }
    }
}
'@
