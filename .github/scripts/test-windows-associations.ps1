$ErrorActionPreference = 'Stop'

# Run only on a disposable CI runner. Seed another viewer and verify that
# registration/removal never replaces it or writes Windows' UserChoice.
$classes = 'HKCU:\Software\Classes'
$pdf = "$classes\.pdf"
$registered = 'HKCU:\Software\RegisteredApplications'
$userChoice = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.pdf\UserChoice'
$choiceBefore = if (Test-Path $userChoice) {
    Get-ItemProperty $userChoice | ConvertTo-Json -Compress
} else { '' }
New-Item -Path "$pdf\OpenWithProgids" -Force | Out-Null
New-ItemProperty -Path $pdf -Name '(default)' -Value 'Other.PDF' -PropertyType String -Force | Out-Null
New-ItemProperty -Path "$pdf\OpenWithProgids" -Name 'Other.PDF' -Value '' -PropertyType String -Force | Out-Null
New-Item -Path $registered -Force | Out-Null
New-ItemProperty -Path $registered -Name 'Other' -Value 'Software\Other\Capabilities' -PropertyType String -Force | Out-Null

$directory = Join-Path $env:RUNNER_TEMP 'Review résumé 日本語 package'
New-Item -ItemType Directory -Path $directory -Force | Out-Null
$executable = Join-Path $directory 'review.exe'
Copy-Item 'target/x86_64-pc-windows-msvc/release/review.exe' $executable
$expectedCommand = '"' + $executable + '" -- "%1"'

function Assert-UnchangedDefaults {
    if ((Get-Item $pdf).GetValue('') -ne 'Other.PDF') {
        throw 'Registration changed the default PDF viewer'
    }
    if ((Get-Item "$pdf\OpenWithProgids").GetValueNames() -notcontains 'Other.PDF') {
        throw 'Registration removed another viewer'
    }
    if ((Get-Item $registered).GetValue('Other') -ne 'Software\Other\Capabilities') {
        throw 'Registration changed another registered application'
    }
    $choiceAfter = if (Test-Path $userChoice) {
        Get-ItemProperty $userChoice | ConvertTo-Json -Compress
    } else { '' }
    if ($choiceAfter -ne $choiceBefore) {
        throw 'Registration changed Windows UserChoice'
    }
}

try {
    foreach ($iteration in 1..2) {
        & ./platform/windows/Install-FileAssociation.ps1 -Executable $executable
        Assert-UnchangedDefaults
        foreach ($key in @("$classes\Review.PDF\shell\open\command", "$classes\Applications\review.exe\shell\open\command")) {
            if ((Get-Item $key).GetValue('') -ne $expectedCommand) {
                throw "Incorrect quoted open command at $key"
            }
        }
        if ((Get-Item "$pdf\OpenWithProgids").GetValueNames() -notcontains 'Review.PDF') {
            throw 'Review is missing from PDF Open With registration'
        }
        if ((Get-Item 'HKCU:\Software\Review\Capabilities\FileAssociations').GetValue('.pdf') -ne 'Review.PDF') {
            throw 'Review PDF capability is missing'
        }
        if ((Get-Item $registered).GetValue('Review') -ne 'Software\Review\Capabilities') {
            throw 'Review is missing from registered applications'
        }
    }
    foreach ($iteration in 1..2) {
        & ./platform/windows/Uninstall-FileAssociation.ps1
        Assert-UnchangedDefaults
        foreach ($key in @("$classes\Review.PDF", "$classes\Applications\review.exe", 'HKCU:\Software\Review\Capabilities')) {
            if (Test-Path $key) { throw "Uninstall left registration at $key" }
        }
        if ((Get-Item "$pdf\OpenWithProgids").GetValueNames() -contains 'Review.PDF' -or
            (Get-Item $registered).GetValueNames() -contains 'Review') {
            throw 'Uninstall left Review in PDF associations'
        }
    }
    Write-Output 'PASS: Windows registration/removal preserve defaults, UserChoice, and Unicode executable paths'
} finally {
    & ./platform/windows/Uninstall-FileAssociation.ps1
    Remove-Item $directory -Recurse -Force
}
