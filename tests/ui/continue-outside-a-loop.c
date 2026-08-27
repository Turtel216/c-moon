// `continue` written after the loop has already been left behind.

int main() {
    while (1) {
        break;
    }
    continue;
    return 0;
}
