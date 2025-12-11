its a "infrastructure" api - I removed the layer between binaries. Different modules in one binary is now the same as different binaries in different countries.



I hate that we are using human readable strings for error handling - all error messages in cell must be compile time checked and you must be able to handle every case. Between cells you should have error types to handle cell specific errors.