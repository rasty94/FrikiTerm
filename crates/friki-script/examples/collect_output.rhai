// Ejecuta un comando y guarda su salida en un fichero dentro de un directorio con permiso
// de escritura. Argumentos: comando fichero_salida
if crt.Arguments.len() < 2 {
    crt.Dialog.MessageBox("uso: collect_output <comando> <fichero>");
    crt.Quit(2);
}
let cmd = crt.Arguments[0];
let path = crt.Arguments[1];

crt.Screen.Send(cmd + "\r");
let out = crt.Screen.ReadString("$ ", 60);
if out == "" {
    crt.Dialog.MessageBox("sin salida (timeout)");
    crt.Quit(1);
}
crt.FileSystem.Write(path, out);
crt.Dialog.MessageBox("salida guardada en " + path);
