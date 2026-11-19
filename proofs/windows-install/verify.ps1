#Requires -PSEdition Core
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string] $Package,
    [Parameter(Mandatory)][string] $Jdk,
    [switch] $UnicodePath
)
$ErrorActionPreference = 'Stop'
if (!$IsWindows -or [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne 'X64') {
    throw 'Run in x64 Windows PowerShell 7+.'
}
$packageRoot = (Get-Item -LiteralPath $Package).FullName
$cli = Join-Path $packageRoot 'luabuddy.exe'
function Invoke-LuaBuddyCli([string[]]$Argv) {
    $text = & $cli @Argv
    if ($LASTEXITCODE -ne 0) { throw "luabuddy $($Argv[0]) failed (exit $LASTEXITCODE)" }
    return ($text -join "`n" | ConvertFrom-Json)
}
function Check([bool]$Ok,[string]$Message) {
    if (!$Ok) { throw $Message }
}
function Hash([string]$Path) { return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash }
$root = Join-Path ([IO.Path]::GetTempPath()) ('LuaBuddy install ' + [Guid]::NewGuid().ToString('N'))
$library = if ($UnicodePath) { 'Steam library 雪' } else { 'Steam library' }
$game = Join-Path $root "$library/steamapps/common/ProjectZomboid"
$site = Join-Path $game 'ProjectZomboid64.site.json'
$base = Join-Path $game 'ProjectZomboid64.json'
$receipt = Join-Path $game '.luabuddy/native-install.json'
$setupGuide = Join-Path $packageRoot 'WINDOWS-SETUP.md'
$before = (Invoke-LuaBuddyCli @('verify-package','--package',$packageRoot))
Check ($before.ok -and !$before.runtimeVerified) 'Manifest verification incorrectly claimed runtime acceptance'
Check (Test-Path -LiteralPath $setupGuide -PathType Leaf) 'Windows package lacks setup instructions'
[void][IO.Directory]::CreateDirectory($game)
try {
    # A real JDK plus inert layout sentinels, NOT the commercial launcher or game.
    foreach ($sentinel in 'projectzomboid.jar','ProjectZomboid64.exe') { [IO.File]::WriteAllBytes((Join-Path $game $sentinel),[byte[]]@()) }
    [void](New-Item -ItemType Junction -Path (Join-Path $game 'jre64') -Target (Get-Item -LiteralPath $Jdk).FullName)
    [IO.File]::WriteAllText($base,'{"vmArgs":["-Xmx512m","-Dunchanged=keep spaces"],"mainClass":"fixture/main","classpath":["base.jar"]}')
    $baseHash = Hash $base
    $packageDll = Join-Path $packageRoot 'luabuddy_host.dll'
    $packageHash = Hash $packageDll
    $argsFirst = @('--package',$packageRoot,'--game-dir',$game)
    $installed = Invoke-LuaBuddyCli (@('install') + $argsFirst)
    Check ($installed.changed -and !$installed.runtimeVerified) 'Installer did not record the new generation'
    $profileDir = $installed.profiles
    Check (@(Get-ChildItem -LiteralPath $profileDir -Force).Count -eq 0) 'Fresh install selected a profile'
    $siteJson = Get-Content -LiteralPath $site -Raw | ConvertFrom-Json
    $agents = @($siteJson.vmArgs | Where-Object { $_ -clike '-agentpath:*' })
    Check ($agents.Count -eq 1) 'Site file must contain one agent argument, not a reconstructed shell command'
    $dll = $agents[0].Substring('-agentpath:'.Length)
    Check (($dll -ceq $installed.runtime) -and ((Hash $dll) -ceq $packageHash)) 'Active DLL differs from the verified package'
    Check ($siteJson.vmArgs -ccontains '-Dunchanged=keep spaces') 'Unrelated VM option lost'
    Check ((Hash $base) -ceq $baseHash) 'Base launcher changed'
    $check = Invoke-LuaBuddyCli @('verify','--game-dir',$game)
    Check ($check.ok -and !$check.runtimeVerified -and ($check.jvm.architecture -ceq 'x86_64')) 'Incorrect installed file/JVM status'
    Check (!(Invoke-LuaBuddyCli (@('install') + $argsFirst)).changed) 'Reinstallation was not idempotent'

    & (Join-Path $PSScriptRoot '../windows-jvm/verify.ps1') -Runtime $dll -Jdk $Jdk

    $sentinel = Join-Path $profileDir 'Personal.properties'
    [IO.File]::WriteAllText($sentinel,"id=Personal`nenabled=false`n")
    $sentinelHash = Hash $sentinel
    $newTemplates = Join-Path $root 'new templates'
    $newPackage = Join-Path $root 'new package'
    [void][IO.Directory]::CreateDirectory($newTemplates)
    [IO.File]::WriteAllText((Join-Path $newTemplates 'Fixture.properties'),"id=Fixture`n")
    $null = Invoke-LuaBuddyCli @('package','--runtime',$packageDll,'--profiles-dir',$newTemplates,'--out',$newPackage)
    $update = Invoke-LuaBuddyCli @('update','--package',$newPackage,'--game-dir',$game)
    Check (($update.runtime -cne $dll) -and ((Hash $update.runtime) -ceq $packageHash)) 'Update did not select distinct package generation'
    $null = Invoke-LuaBuddyCli @('rollback','--game-dir',$game)
    Check ((Invoke-LuaBuddyCli @('verify','--game-dir',$game)).runtime -ceq $dll) 'Rollback selected a different DLL'
    $null = Invoke-LuaBuddyCli @('uninstall','--game-dir',$game)
    Check (!(Test-Path -LiteralPath $site) -and !(Test-Path -LiteralPath $receipt)) 'Removal left an override or receipt'
    Check (!(Test-Path -LiteralPath $dll) -and !(Test-Path -LiteralPath $update.runtime)) 'Removal left a generation DLL'
    Check (!(Test-Path -LiteralPath (Join-Path $game '.luabuddy/native-install.lock'))) 'Removal left an active installer lock'
    Check (((Hash $base) -ceq $baseHash) -and ((Hash $sentinel) -ceq $sentinelHash)) 'Removal changed vendor config or personal profile'
    Check (!(Invoke-LuaBuddyCli @('uninstall','--game-dir',$game)).changed) 'Repeated uninstall was not a no-op'
    Write-Host "PASS Windows release package, empty selection, installed DLL startup, update/rollback, reversible uninstall, preserved base and personal files; SHA256=$packageHash"
} finally {
    # Remove the test-only directory junction itself before cleaning our fixture.
    $junction = Join-Path $game 'jre64'
    if (Test-Path -LiteralPath $junction) { [IO.Directory]::Delete($junction) }
    [IO.Directory]::Delete($root,$true)
}
