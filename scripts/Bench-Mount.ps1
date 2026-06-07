Start-Process -FilePath pwsh -WorkingDirectory (Get-Item .).FullName -ArgumentList `
    '-Command',
    'cargo run --release -- dvdbnd -k dist/dvdbnd/Key -d dist/dvdbnd/Hash -m G:/fsvfs/eldenring `
        ''D:\Steam\steamapps\common\ELDEN RING\Game\Data3.bhd''
        
        Write-Host "`nPress any key to continue..."
        $null = $Host.UI.RawUI.ReadKey(''NoEcho,IncludeKeyDown'')'
