#Requires -PSEdition Core
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string] $Runtime,
    [Parameter(Mandatory)][string] $Jdk
)
$ErrorActionPreference = 'Stop'
if (!$IsWindows -or [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne 'X64') {
    throw 'This proof must run in native x64 Windows PowerShell 7+.'
}
$runtimeFile = Get-Item -LiteralPath $Runtime -ErrorAction Stop
# Do not rewrite forward slashes or an extended-path prefix from a generated
# launcher option. Relative paths are supported only as a standalone convenience.
$dll = if ([IO.Path]::IsPathFullyQualified($Runtime)) { $Runtime } else { $runtimeFile.FullName }
$jdkRoot = (Get-Item -LiteralPath $Jdk -ErrorAction Stop).FullName
$java = Join-Path $jdkRoot 'bin/java.exe'
$javac = Join-Path $jdkRoot 'bin/javac.exe'
$root = Join-Path ([IO.Path]::GetTempPath()) ('LuaBuddy JVM proof ' + [Guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($root)

# Capture both streams while waiting; kill only our own disposable process on timeout.
function Invoke-Checked([string] $Program, [string[]] $CommandArgs, [hashtable] $Vars = @{}) {
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $Program
    $info.WorkingDirectory = $root
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    foreach ($arg in $CommandArgs) { $info.ArgumentList.Add($arg) }
    foreach ($name in 'JAVA_TOOL_OPTIONS','JDK_JAVA_OPTIONS','_JAVA_OPTIONS','CLASSPATH') {
        [void]$info.Environment.Remove($name)
    }
    foreach ($key in $Vars.Keys) { $info.Environment[$key] = $Vars[$key] }
    $p = [Diagnostics.Process]::Start($info)
    try {
        $stdout = $p.StandardOutput.ReadToEndAsync()
        $stderr = $p.StandardError.ReadToEndAsync()
        if (!$p.WaitForExit(60000)) { $p.Kill($true); $p.WaitForExit(); throw "Timed out: $Program" }
        $log = $stdout.GetAwaiter().GetResult() + [Environment]::NewLine + $stderr.GetAwaiter().GetResult()
        if ($p.ExitCode -ne 0) { throw "$Program returned $($p.ExitCode): $log" }
        return $log
    } finally { $p.Dispose() }
}
function Assert-Markers([string] $Log, [string[]] $Patterns) {
    foreach ($pattern in $Patterns) {
        if ($Log -cnotmatch $pattern) { throw "Missing proof marker '$pattern': $Log" }
    }
}

try {
    # Exercise the exact installed path supplied by the caller, never a copy
    # or a similarly named binary found through PATH.
    $hash = (Get-FileHash -LiteralPath $dll -Algorithm SHA256).Hash
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $devcmd = (Invoke-Checked $vswhere @('-latest','-products','*','-requires','Microsoft.VisualStudio.Component.VC.Tools.x86.x64','-find','Common7\Tools\VsDevCmd.bat')).Trim()
    if (!(Test-Path -LiteralPath $devcmd -PathType Leaf)) { throw 'VS x64 MSVC developer tools not found' }
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'header-layout.c') -Destination $root
    @'
@echo off
call "%LB_VSDEVCMD%" -no_logo -arch=x64 -host_arch=x64
if errorlevel 1 exit /b 1
cd /d "%~dp0" || exit /b 1
cl.exe /nologo /std:c11 /W4 /WX /I"%LB_JDK%\include" /I"%LB_JDK%\include\win32" header-layout.c /Fe:header-layout.exe
if errorlevel 1 exit /b 1
header-layout.exe
'@ | Set-Content -LiteralPath (Join-Path $root 'check-headers.cmd') -Encoding ascii
    $headers = Invoke-Checked $env:ComSpec @('/d','/c','check-headers.cmd') @{ LB_JDK=$jdkRoot; LB_VSDEVCMD=$devcmd }
    Assert-Markers $headers @('PASS JDK JNI/JVMTI x64 header layout and capability bitfields')
    Write-Host $headers.Trim()

    $classes = Join-Path $root 'empty classpath'
    [void][IO.Directory]::CreateDirectory($classes)
    $argsJvm = @('-Xverify:all','-Xcheck:jni',"-Duser.home=$root",'-cp',$classes,"-agentpath:$dll",'-XshowSettings:properties','-version')
    $missing = Invoke-Checked $java $argsJvm
    $platform = @('(?m)^\s*java\.specification\.version = 25\s*$','(?m)^\s*os\.arch = (amd64|x86_64)\s*$','(?m)^\s*os\.name = Windows','(?m)^\s*sun\.arch\.data\.model = 64\s*$')
    Assert-Markers $missing ($platform + @('\[LuaBuddy\] agent\.startup: Inactive:', 'NoClassDefFoundError: zombie/Lua/Event'))

    # Inert input, not an implementation of any game method. Its presence proves
    # ClassFileLoadHook reached the agent and refused the unrecognized layout.
    $source = Join-Path $root 'Event.java'
    'package zombie.Lua; public final class Event {}' | Set-Content -LiteralPath $source -Encoding ascii
    [void](Invoke-Checked $javac @('-proc:none','-d',$classes,$source))
    $unknown = Invoke-Checked $java $argsJvm
    Assert-Markers $unknown ($platform + @('\[LuaBuddy native bridge\] rejected: host bridge target method not found','\[LuaBuddy\] agent\.startup: Inactive: native event dispatcher was not registered'))
    foreach ($log in @($missing,$unknown)) {
        if ($log -match 'WARNING in native method|FATAL ERROR in native method|JNI DETECTED ERROR|\[LuaBuddy\] agent\.started:|panic contained|Rust runtime inactive:') {
            throw "Unexpected JVM or product state: $log"
        }
    }
    if ((Get-FileHash -LiteralPath $dll -Algorithm SHA256).Hash -cne $hash) { throw 'Installed DLL changed during proof' }
    $version = ($missing -split '\r?\n' | Where-Object { $_ -match '^\s*java.runtime.version =' } | Select-Object -First 1).Trim()
    Write-Host "PASS production Agent_OnLoad, VMInit, class-load ABI, missing and unknown target rejection; $version; SHA256=$hash (stock JVM, not Project Zomboid or Steam)."
} finally {
    [IO.Directory]::Delete($root, $true)
}
