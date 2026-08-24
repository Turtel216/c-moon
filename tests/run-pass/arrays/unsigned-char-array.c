// Elements of an `unsigned char[]` are bytes that read as 0 to 255.
//@ exit-code: 255

int main() {
    unsigned char bytes[3];
    bytes[0] = 255;
    bytes[1] = 128;
    bytes[2] = 1;

    unsigned int sum = 0;
    int i = 0;
    while (i < 3) {
        sum = sum + bytes[i];
        i = i + 1;
    }
    if (sum != 384) {
        return 1;
    }

    unsigned char *p = &bytes[0];
    return *p;
}
