//! Text the firmware writes itself (status lines, errors, prompts), in the
//! screen's language. Text in the `.slint` files is translated by Slint
//! (`@tr`, ui/translations/pt/LC_MESSAGES/esp32-screen.po).
//!
//! `tr` looks up an English string; `trf` also fills `{}` placeholders, in
//! order. Missing translations show in English.

use std::sync::atomic::{AtomicBool, Ordering};

static PORTUGUESE: AtomicBool = AtomicBool::new(false);

pub fn set_portuguese(on: bool) {
    PORTUGUESE.store(on, Ordering::Relaxed);
}

pub fn portuguese() -> bool {
    PORTUGUESE.load(Ordering::Relaxed)
}

/// `en`, or its Portuguese (Portugal) translation when that is the language.
pub fn tr(en: &'static str) -> &'static str {
    if !portuguese() {
        return en;
    }
    PT.iter()
        .find(|(e, _)| *e == en)
        .map(|(_, p)| *p)
        .unwrap_or(en)
}

/// `tr(template)` with each `{}` replaced by the next argument.
pub fn trf(template: &'static str, args: &[&dyn std::fmt::Display]) -> String {
    fill(tr(template), args)
}

fn fill(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut args = args.iter();
    let mut rest = template;
    while let Some(i) = rest.find("{}") {
        out.push_str(&rest[..i]);
        if let Some(arg) = args.next() {
            out.push_str(&arg.to_string());
        }
        rest = &rest[i + 2..];
    }
    out.push_str(rest);
    out
}

/// English → Portuguese (Portugal).
pub const PT: &[(&str, &str)] = &[
    // Wi-Fi setup
    ("Step 1 of 2 · Wi-Fi", "Passo 1 de 2 · Wi-Fi"),
    ("Step 2 of 2 · Home Assistant", "Passo 2 de 2 · Home Assistant"),
    ("Using the saved network.", "A usar a rede guardada."),
    ("Connecting to {}…", "A ligar a {}…"),
    ("Network name", "Nome da rede"),
    ("{} · Enter the exact name (SSID)", "{} · Introduza o nome exato (SSID)"),
    ("Next", "Seguinte"),
    ("Password for {}", "Palavra-passe de {}"),
    ("At least 8 characters", "Pelo menos 8 caracteres"),
    ("Leave empty for an open network", "Deixe vazio numa rede aberta"),
    ("Password", "Palavra-passe"),
    ("Connect", "Ligar"),
    ("This can take a few seconds.", "Pode demorar alguns segundos."),
    ("Cancel", "Cancelar"),
    (
        "No networks found. Move closer to the router, or enter the name by hand.",
        "Nenhuma rede encontrada. Aproxime-se do router ou introduza o nome à mão.",
    ),
    ("Couldn't scan for networks ({}).", "Não foi possível procurar redes ({})."),
    ("Attempt {}…", "Tentativa {}…"),
    ("{} · IP address {}", "{} · Endereço IP {}"),
    ("Connected", "Ligado"),
    ("Continue", "Continuar"),
    ("Reconnecting… {}", "A religar… {}"),
    ("Can't reach {}", "Não é possível chegar a {}"),
    ("{} Trying again in {} s.", "{} Nova tentativa dentro de {} s."),
    ("Choose another network", "Escolher outra rede"),
    (" Passwords are case-sensitive.", " As palavras-passe distinguem maiúsculas de minúsculas."),
    ("Couldn't connect to {}", "Não foi possível ligar a {}"),
    ("Try again", "Tentar novamente"),
    ("The password seems to be wrong.", "A palavra-passe parece estar errada."),
    (
        "The network is out of range or the name is wrong.",
        "A rede está fora de alcance ou o nome está errado.",
    ),
    (
        "The network uses a security mode this screen doesn't support.",
        "A rede usa um modo de segurança que este ecrã não suporta.",
    ),
    ("The network didn't respond in time.", "A rede não respondeu a tempo."),
    ("The connection was lost.", "A ligação perdeu-se."),
    ("Wi-Fi error (reason {}).", "Erro de Wi-Fi (motivo {})."),
    ("Busy connecting", "Ocupado a ligar"),
    ("Wi-Fi offline", "Wi-Fi desligado"),
    ("Waiting for Wi-Fi.", "À espera do Wi-Fi."),
    ("{} signal · {} dBm", "Sinal {} · {} dBm"),
    ("Excellent", "excelente"),
    ("Good", "bom"),
    ("Fair", "razoável"),
    ("Weak", "fraco"),
    // Home Assistant sign-in
    ("Home Assistant address", "Endereço do Home Assistant"),
    ("{} · For example {}", "{} · Por exemplo {}"),
    ("Sign in to Home Assistant", "Iniciar sessão no Home Assistant"),
    ("Username", "Nome de utilizador"),
    ("Sign in", "Iniciar sessão"),
    ("Two-factor code", "Código de dois fatores"),
    (
        "{} · Enter the code from your authenticator app",
        "{} · Introduza o código da aplicação de autenticação",
    ),
    ("Code", "Código"),
    ("Verify", "Verificar"),
    ("Checking the code…", "A verificar o código…"),
    ("Couldn't sign in", "Não foi possível iniciar sessão"),
    ("Change address", "Alterar endereço"),
    ("Check the certificate", "Verifique o certificado"),
    (
        "Home Assistant's certificate isn't from a known authority. It may be self-signed. Trust it only if its SHA-256 fingerprint is:\n\n{}",
        "O certificado do Home Assistant não é de uma autoridade conhecida. Pode ser autoassinado. Confie nele só se a impressão digital SHA-256 for:\n\n{}",
    ),
    ("Trust", "Confiar"),
    ("Signed in to Home Assistant", "Sessão iniciada no Home Assistant"),
    (
        "This screen shows up in Home Assistant as \u{201c}{}\u{201d}. Design its layout under Screens in the sidebar.",
        "Este ecrã aparece no Home Assistant como \u{201c}{}\u{201d}. Desenhe o painel em Screens, na barra lateral.",
    ),
    ("Couldn't reach Home Assistant ({}).", "Não foi possível chegar ao Home Assistant ({})."),
    ("Wrong username or password.", "Nome de utilizador ou palavra-passe errados."),
    ("That code is not valid.", "Esse código não é válido."),
    (
        "Home Assistant asked for an unsupported login step.",
        "O Home Assistant pediu um passo de início de sessão não suportado.",
    ),
    ("Unexpected reply from Home Assistant.", "Resposta inesperada do Home Assistant."),
    ("The login expired. Start again.", "O início de sessão expirou. Recomece."),
    // Home Assistant status
    ("Not set up", "Não configurado"),
    ("Connecting…", "A ligar…"),
    ("Offline · {}", "Sem ligação · {}"),
    ("Signed out", "Sessão terminada"),
    ("Integration not installed", "Integração não instalada"),
    ("Home Assistant isn't set up", "O Home Assistant não está configurado"),
    (
        "Connect this screen to Home Assistant to show your devices.",
        "Ligue este ecrã ao Home Assistant para mostrar os seus dispositivos.",
    ),
    ("Set up", "Configurar"),
    ("Sign in again", "Inicie sessão novamente"),
    ("Install the ESP32 Screen integration", "Instale a integração ESP32 Screen"),
    (
        "In Home Assistant open Settings › Devices & services › Add integration, and add ESP32 Screen.",
        "No Home Assistant abra Definições › Dispositivos e serviços › Adicionar integração e adicione ESP32 Screen.",
    ),
    ("Nothing to show yet", "Ainda nada para mostrar"),
    (
        "In Home Assistant, open Screens in the sidebar and design the layout for \u{201c}{}\u{201d}.",
        "No Home Assistant, abra Screens na barra lateral e desenhe o painel de \u{201c}{}\u{201d}.",
    ),
    ("Can't reach Home Assistant", "Não é possível chegar ao Home Assistant"),
    ("Connecting to Home Assistant…", "A ligar ao Home Assistant…"),
    ("Home Assistant didn't answer in time.", "O Home Assistant não respondeu a tempo."),
    ("Home Assistant is offline.", "O Home Assistant está sem ligação."),
    (
        "Home Assistant no longer accepts this screen's login. Sign in again.",
        "O Home Assistant já não aceita a sessão deste ecrã. Inicie sessão novamente.",
    ),
    ("Can't reach Home Assistant ({}).", "Não é possível chegar ao Home Assistant ({})."),
    ("Can't open a connection ({}).", "Não é possível abrir uma ligação ({})."),
    (
        "Home Assistant's certificate changed. Sign in again to check the new one.",
        "O certificado do Home Assistant mudou. Inicie sessão novamente para verificar o novo.",
    ),
    ("Sending failed ({}).", "O envio falhou ({})."),
    ("Connection lost.", "Ligação perdida."),
    ("Login expired, renewing.", "Sessão expirada, a renovar."),
    ("Home Assistant refused the screen ({}).", "O Home Assistant recusou o ecrã ({})."),
    // Dashboard and pop-ups
    ("{} is unavailable.", "{} está indisponível."),
    ("Enter the code first.", "Introduza primeiro o código."),
    ("Open", "Abrir"),
    ("Stop", "Parar"),
    ("Close", "Fechar"),
    ("Position", "Posição"),
    ("Unlock", "Destrancar"),
    ("Lock", "Trancar"),
    ("Disarm", "Desarmar"),
    ("Home", "Casa"),
    ("Away", "Ausente"),
    ("Night", "Noite"),
    ("Vacation", "Férias"),
    ("Previous", "Anterior"),
    ("Pause", "Pausa"),
    ("Play", "Reproduzir"),
    ("Power", "Ligar/desligar"),
    ("Volume", "Volume"),
    ("Off", "Desligado"),
    ("On", "Ligado"),
    ("Cool", "Arrefecer"),
    ("Heat", "Aquecer"),
    ("Heat/Cool", "Aquecer/Arrefecer"),
    ("Auto", "Automático"),
    ("Dry", "Desumidificar"),
    ("Fan", "Ventilação"),
    // Doorbell, notifications, updates
    ("Test video", "Vídeo de teste"),
    ("Connecting to the camera…", "A ligar à câmara…"),
    ("The video stopped", "O vídeo parou"),
    ("No video from the camera", "Sem vídeo da câmara"),
    ("Updating to {}", "A atualizar para {}"),
    (
        "Installing from {}. Don't unplug the screen.",
        "A instalar a partir de {}. Não desligue o ecrã.",
    ),
    ("Checking the new firmware…", "A verificar o novo firmware…"),
    ("Restarting…", "A reiniciar…"),
    ("Update failed", "A atualização falhou"),
    ("{}. The screen keeps running {}.", "{}. O ecrã continua com a versão {}."),
    ("SD card", "cartão SD"),
    // Weather
    ("Today", "Hoje"),
    ("No weather forecast", "Sem previsão do tempo"),
    (
        "Pick a weather entity for this screen in Home Assistant: Screens › Screen settings.",
        "Escolha uma entidade de meteorologia para este ecrã no Home Assistant: Screens › Screen settings.",
    ),
    // Display settings
    (
        "Light from sunrise ({}) to sunset ({}), from Home Assistant",
        "Claro do nascer ({}) ao pôr do sol ({}), segundo o Home Assistant",
    ),
    (
        "Light while Home Assistant's sun is up",
        "Claro enquanto o sol do Home Assistant estiver acima do horizonte",
    ),
    (
        "Light from 07:00 to 19:00 until Home Assistant sends sunrise and sunset",
        "Claro das 07:00 às 19:00 até o Home Assistant enviar o nascer e o pôr do sol",
    ),
    ("{}. {}", "{}. {}"),
    ("Now: light.", "Agora: claro."),
    ("Now: dark.", "Agora: escuro."),
    ("Dark until the time is known.", "Escuro até se saber a hora."),
    ("{} free of {}", "{} livres de {}"),
    ("Mounted", "Montado"),
    ("None (insert one and restart)", "Nenhum (insira um e reinicie)"),
    // PIN
    ("Enter the PIN to open the settings", "Introduza o PIN para abrir as definições"),
    ("Enter the PIN", "Introduza o PIN"),
    ("Choose a PIN", "Escolha um PIN"),
    ("4 to 8 digits.", "4 a 8 algarismos."),
    ("Enter the new PIN again", "Introduza o novo PIN outra vez"),
    ("Wrong PIN. {} tries left before a wait.", "PIN errado. Faltam {} tentativas antes de uma espera."),
    ("Wrong PIN. 1 try left before a wait.", "PIN errado. Falta 1 tentativa antes de uma espera."),
    ("Too many wrong PINs. Try again in {} s.", "Demasiados PIN errados. Tente de novo dentro de {} s."),
    ("Use 4 to 8 digits.", "Use 4 a 8 algarismos."),
    (
        "The two PINs were different. Choose a PIN.",
        "Os dois PIN eram diferentes. Escolha um PIN.",
    ),
];
