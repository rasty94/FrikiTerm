// Login con espera de prompt. Argumentos: usuario contraseña (nunca literales en el script).
if crt.Arguments.len() < 2 {
    crt.Dialog.MessageBox("uso: login <usuario> <contraseña>");
    crt.Quit(2);
}
let user = crt.Arguments[0];
let pass = crt.Arguments[1];

if !crt.Screen.WaitForString("login:", 20) {
    crt.Dialog.MessageBox("no apareció el prompt de usuario");
    crt.Quit(1);
}
crt.Screen.Send(user + "\r");

if !crt.Screen.WaitForString("assword:", 10) {
    crt.Dialog.MessageBox("no apareció el prompt de contraseña");
    crt.Quit(1);
}
crt.Screen.Send(pass + "\r");

// 1 = shell de usuario, 2 = shell de root, 3 = rechazo, 0 = timeout
let r = crt.Screen.WaitForStrings(["$ ", "# ", "incorrect"], 15);
if r == 0 || r == 3 {
    crt.Dialog.MessageBox("login fallido");
    crt.Quit(1);
}
crt.Dialog.MessageBox("sesión iniciada");
