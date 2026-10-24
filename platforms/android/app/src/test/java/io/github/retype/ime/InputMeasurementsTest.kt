package io.github.retype.ime
import org.junit.Assert.*
import org.junit.Test
class InputMeasurementsTest {
    @Test fun displayGroupsAggregateExactSamplesAndPreserveTailUnits() {
        var input=InputMeasurements()
        for(n in listOf(1,2,3,4,5,9,10,11,19,20,27)) {
            val rows=MutableList(20){LengthCounts()};rows[n.coerceAtMost(20)-1]=LengthCounts(1,if(n%2==0)1 else 0,n.toLong())
            input+=InputMeasurements(chinese=rows)
        }
        assertEquals(listOf(1L,1L,1L,1L,3L,4L),input.grouped(true).map {it.inputs})
        assertEquals(47L,input.chinese[19].units)
        assertEquals(77L,input.grouped(true)[5].units)
        assertEquals(11L,input.total(true).inputs)
        assertNull(LengthCounts().rate())
        assertEquals(100.0/3,LengthCounts(3,1,24).rate()!!,0.0001)
    }
}
