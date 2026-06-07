Measure-Command {
    Get-ChildItem -Path G:\fsvfs\eldenring -Exclude .* | % {
        Copy-Item -Path $_.FullName -Destination G:\fsvfs\eldenring-unpacked -Recurse
    }
}
| Select-Object -ExpandProperty TotalSeconds
| % { Write-Output "$(2708.0 / $_) MB/s" }

Get-ChildItem -Path G:\fsvfs\eldenring-unpacked | % {
    Remove-Item -Path $_.FullName -Recurse -Force
}
