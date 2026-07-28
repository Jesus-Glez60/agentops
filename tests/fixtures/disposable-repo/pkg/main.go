package main

import (
	"fmt"
	"os"
)

func Add(a int, b int) int {
	return a + b
}

func main() {
	fmt.Println(Add(2, 3))
	_ = os.Args
}
