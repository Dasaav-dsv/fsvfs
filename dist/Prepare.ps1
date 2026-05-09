New-Item -Path dist\dvdbnd -ItemType Directory -Force

New-Item -Path dist\dvdbnd\Hash -ItemType Directory -Force
New-Item -Path dist\dvdbnd\Key -ItemType Directory -Force

Copy-Item -Path BinderKeys\LICENSE -Destination dist\dvdbnd\LICENSE -Force

Get-ChildItem -Path BinderKeys -Recurse -Directory | ForEach-Object {
    if ( ( $_.Name -ieq "Hash" ) -or ( $_.Name -ieq "Key" ) ) {
        $Parent = Split-Path -Path "$($_.FullName)" -Parent | Split-Path -Leaf
        $Dest = "dist\dvdbnd\$($_.Name)\$Parent"

        New-Item -Path $Dest -ItemType Directory -Force
        Copy-Item -Path "$($_.FullName)\*" -Destination $Dest -Force
    }
}
