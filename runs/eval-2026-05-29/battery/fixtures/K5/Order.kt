package shop

// Decoy: a different data class.
data class Order(
    val id: Int,
    val lines: List<String>,
)

data class Customer(val name: String, val email: String)
