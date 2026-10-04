param([string]$Executable = (Join-Path $PSScriptRoot 'review.exe'))
$ErrorActionPreference = 'Stop'

# Per-user registration only. Windows, not this script, manages UserChoice.
$Executable = (Resolve-Path -LiteralPath $Executable).ProviderPath
if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
    throw "Expected Review executable at $Executable"
}
$classes = 'HKCU:\Software\Classes'
$command = '"' + $Executable + '" -- "%1"'

function Set-StringValue($Path, $Name, $Value) {
    if (-not (Test-Path -LiteralPath $Path)) {
        New-Item -Path $Path -Force | Out-Null
    }
    New-ItemProperty -Path $Path -Name $Name -Value $Value -PropertyType String -Force | Out-Null
}

Set-StringValue "$classes\Review.PDF" '(default)' 'PDF document'
Set-StringValue "$classes\Review.PDF\DefaultIcon" '(default)' ($Executable + ',0')
Set-StringValue "$classes\Review.PDF\shell\open\command" '(default)' $command
Set-StringValue "$classes\.pdf\OpenWithProgids" 'Review.PDF' ''
Set-StringValue "$classes\Applications\review.exe" 'FriendlyAppName' 'Review'
Set-StringValue "$classes\Applications\review.exe\DefaultIcon" '(default)' ($Executable + ',0')
Set-StringValue "$classes\Applications\review.exe\SupportedTypes" '.pdf' ''
Set-StringValue "$classes\Applications\review.exe\shell\open\command" '(default)' $command

$capabilities = 'HKCU:\Software\Review\Capabilities'
Set-StringValue $capabilities 'ApplicationName' 'Review'
Set-StringValue $capabilities 'ApplicationDescription' 'Read PDF documents'
Set-StringValue "$capabilities\FileAssociations" '.pdf' 'Review.PDF'
Set-StringValue 'HKCU:\Software\RegisteredApplications' 'Review' 'Software\Review\Capabilities'

if (-not ('Review.Shell' -as [type])) {
    Add-Type -Namespace Review -Name Shell -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("shell32.dll")]
public static extern void SHChangeNotify(uint eventId, uint flags, System.IntPtr item1, System.IntPtr item2);
'@
}
[Review.Shell]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output 'Registered Review for PDFs. Choose it in Open with or Settings > Apps > Default apps.'
Write-Output "Your default viewer is unchanged. Keep the executable at: $Executable"
