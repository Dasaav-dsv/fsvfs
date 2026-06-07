Measure-Command {
    Get-ChildItem -Path G:\fsvfs\eldenring -Exclude .* -Name | % {
        robocopy G:\fsvfs\eldenring\$_ G:\fsvfs\eldenring-unpacked\$_ /e /mt
    }
}
| Select-Object -ExpandProperty TotalSeconds
| % { Write-Output "$(2708.0 / $_) MB/s" }

Get-ChildItem -Path G:\fsvfs\eldenring-unpacked | % {
    Remove-Item -Path $_.FullName -Recurse -Force
}
