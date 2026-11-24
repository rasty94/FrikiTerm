// Ejecuta una lista de comandos (cada argumento es un comando) esperando el prompt "$ ".
if crt.Arguments.len() == 0 {
    crt.Dialog.MessageBox("uso: run_commands <comando> [<comando> ...]");
    crt.Quit(2);
}
for cmd in crt.Arguments {
    crt.Screen.Send(cmd + "\r");
    if !crt.Screen.WaitForString("$ ", 30) {
        crt.Dialog.MessageBox("timeout esperando el prompt tras: " + cmd);
        crt.Quit(1);
    }
}
