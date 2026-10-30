# Windows ругается на Snowdrift: SmartScreen и Defender

## Коротко

Windows предупреждает не потому, что в программе вирус, а потому что EXE:

- скачан из интернета;
- не подписан доверенным сертификатом;
- имеет низкую репутацию у Microsoft.

SmartScreen показывает «Windows protected your PC» / «Windows защитила ваш компьютер», а Defender может добавить эвристическое срабатывание. Установщик Snowdrift вдобавок распаковывает внутри себя приложение, прописывает автозапуск и умеет скачивать обновления — для антивируса это выглядит как поведение дроппера, пока файл не подписан нормальным сертификатом.

**Самоподписанный сертификат (`New-SelfSignedCertificate`) проблему не решает.** Windows не доверяет ему, поэтому подпись выглядит как «Unknown publisher» и может даже усиливать подозрения. Именно так сейчас подписаны релизы: `CN=Snowdrift, O=Snowdrift`, выдан сам себе.

## Что сделать, чтобы предупреждений не было у пользователей

Нужен настоящий code-signing сертификат от доверенного центра. Варианты:

| Вариант | Цена | Комментарий |
| --- | --- | --- |
| [SignPath Foundation](https://signpath.org/) | бесплатно для open source | Лучший вариант для этого проекта: настоящий сертификат, подпись в GitHub Actions. Нужно подать заявку и дождаться одобрения. |
| [Certum Open Source Code Signing](https://certum.store/) | ~€100/год | Дешёвый сертификат для OSS, есть cloud signing. |
| [Azure Trusted Signing](https://azure.microsoft.com/products/trusted-signing) | ~$10/мес | Нужна подписка Azure и проверка личности. Хорошо интегрируется с GitHub Actions. |
| OV/EV сертификат (DigiCert, Sectigo, GlobalSign) | от ~$200/год | EV даёт доверие SmartScreen сразу, OV набирает репутацию со временем. |

Для open-source проекта проще всего начать с SignPath Foundation.

### Если сертификат в виде PFX

1. Положите PFX в GitHub Secrets (не в репозиторий):

   ```powershell
   [Convert]::ToBase64String([IO.File]::ReadAllBytes('C:\path\codesign.pfx'))
   ```

2. В GitHub: **Settings → Secrets and variables → Actions → New repository secret**:

   - `WINDOWS_CERT_PFX_BASE64` — вывод команды выше;
   - `WINDOWS_CERT_PASSWORD` — пароль от PFX.

3. Запустите workflow на новый тег. Он сам:

   - соберёт `Snowdrift.exe`;
   - подпишет его;
   - соберёт `Snowdrift-Setup.exe` уже вокруг подписанного приложения;
   - подпишет установщик;
   - проверит подпись через `signtool verify /pa`;
   - положит в релиз `SHA256SUMS.txt`.

Если секреты не заданы, workflow продолжит работать, но выведет предупреждение «Unsigned Windows build». Это значит, что релиз снова будет ругаться.

> Важно: подписывать приложение нужно **до** сборки установщика. Установщик хранит копию приложения внутри себя (`include_bytes!`), поэтому подпись одного установщика не делает подписанным установленное приложение.

### Если сертификат через SignPath

SignPath подписывает артефакты, загруженные в GitHub Actions. Схема для двух файлов:

1. собрать `Snowdrift.exe`;
2. загрузить его как artifact и отправить в SignPath;
3. скачать подписанный `Snowdrift.exe`;
4. собрать `Snowdrift-Setup.exe` с `SNOWDRIFT_APP`, указывающим на подписанный файл;
5. загрузить установщик как artifact и отправить в SignPath;
6. скачать подписанный установщик и приложить к релизу.

В проекте SignPath понадобится artifact configuration для `Snowdrift.exe` и `Snowdrift-Setup.exe`. В документации SignPath есть готовый action:

```yaml
- uses: signpath/github-action-submit-signing-request@v3
  with:
    api-token: ${{ secrets.SIGNPATH_API_TOKEN }}
    organization-id: ${{ secrets.SIGNPATH_ORGANIZATION_ID }}
    project-slug: snowdrift
    signing-policy-slug: release
    github-artifact-id: ${{ steps.upload-unsigned.outputs.artifact-id }}
    wait-for-completion: true
    output-artifact-directory: signed
```

### Локальная подпись

Если сертификат установлен в `Cert:\CurrentUser\My`:

```powershell
.\scripts\sign-windows.ps1 -Files .\target\release\Snowdrift.exe -Thumbprint <отпечаток>
```

Если есть PFX:

```powershell
.\scripts\sign-windows.ps1 -Files .\target\release\Snowdrift.exe -PfxPath .\codesign.pfx -Password 'пароль'
```

## Локальная разработка: чтобы Defender не съедал свежую сборку

Запустите **от имени администратора**:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\allow-defender.ps1
```

Скрипт добавляет исключения только для:

- `target\release`;
- `dist`;
- `%LOCALAPPDATA%\Programs\Snowdrift`;
- `%TEMP%\Snowdrift-Setup.exe`.

И снимает с локальных EXE метку «скачано из интернета». Отменить:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\allow-defender.ps1 -Remove
```

Это только для вашей машины. Пользователям такое советовать нельзя: исключения снижают защиту. Не отключайте Defender глобально и не просите об этом пользователей.

## Ложное срабатывание Defender

Если Windows Security показывает конкретное имя угрозы (например, `Trojan:Win32/Wacatac.B!ml`), отправьте файл в Microsoft:

1. **Windows Security → Protection history** — запишите точное имя обнаружения.
2. Откройте <https://www.microsoft.com/en-us/wdsi/filesubmission>.
3. Выберите **Software developer**.
4. Укажите продукт **Microsoft Defender Antivirus (Windows 11)** (или **Microsoft Defender SmartScreen**, если блокирует именно SmartScreen).
5. Выберите **Incorrectly detected as malware/malicious**.
6. Укажите detection name, версию определений и приложите **подписанный** EXE.
7. В комментарии напишите, что это open-source приложение для снега на рабочем столе, и дайте ссылку на репозиторий.
8. После обновления определений проверьте файл снова. Обычно ответ приходит за 1–3 дня.

Проверить статус можно на той же странице в **Submission history**.

## Smart App Control

В Windows 11 есть Smart App Control, который блокирует неподписанные приложения независимо от Defender. Проверить:

**Windows Security → App & browser control → Smart App Control**.

Если включён режим Enforcement, неподписанное приложение запустить нельзя. Правильное решение — подпись. Отключение Smart App Control не рекомендуется: обратно включить его без переустановки Windows может не получиться.

## Чего делать не надо

- Не подписывать релизы самоподписанным сертификатом.
- Не отключать антивирус у пользователей.
- Не обфусцировать и не паковать EXE «чтобы обойти антивирус» — эвристика сработает ещё сильнее.
- Не просить пользователей добавлять весь диск в исключения.

## Что делать пользователю прямо сейчас

1. Скачивать только с официальной страницы GitHub Releases.
2. В окне SmartScreen нажать **Подробнее → Выполнить в любом случае**.
3. Проверить SHA-256 из `SHA256SUMS.txt`:

   ```powershell
   Get-FileHash .\Snowdrift-Setup.exe -Algorithm SHA256
   ```

4. Если файл уже удалил Defender — восстановить его из **Protection history** и только потом запускать.

Старые релизы останутся «подозрительными», пока не выйдет новый релиз с нормальной подписью.
