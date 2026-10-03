$ErrorActionPreference = 'Stop'

$classes = 'HKCU:\Software\Classes'
foreach ($path in @("$classes\Review.PDF", "$classes\Applications\review.exe", 'HKCU:\Software\Review\Capabilities')) {
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Recurse -Force
    }
}
foreach ($entry in @(
    @{ Path = "$classes\.pdf\OpenWithProgids"; Name = 'Review.PDF' },
    @{ Path = 'HKCU:\Software\RegisteredApplications'; Name = 'Review' }
)) {
    if (Test-Path -LiteralPath $entry.Path) {
        $key = Get-Item -LiteralPath $entry.Path
        if ($key.GetValueNames() -contains $entry.Name) {
            Remove-ItemProperty -LiteralPath $entry.Path -Name $entry.Name
        }
    }
}

if (-not ('Review.Shell' -as [type])) {
    Add-Type -Namespace Review -Name Shell -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("shell32.dll")]
public static extern void SHChangeNotify(uint eventId, uint flags, System.IntPtr item1, System.IntPtr item2);
'@
}
[Review.Shell]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output "Removed Review's PDF registration. If it was your default, choose another PDF viewer."
