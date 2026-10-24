#import "App.h"
#include <math.h>

static NSColor *Bar(void){return [NSColor colorWithCalibratedRed:19/255. green:143/255. blue:150/255. alpha:1];}
static NSColor *Line(void){return [NSColor colorWithCalibratedRed:218/255. green:123/255. blue:47/255. alpha:1];}
static NSArray<NSString *> *Labels(void){return @[@"1",@"2",@"3",@"4",@"5–10",@"11+"];}
static NSString *Rate(double n,double corrected){return n>0?[NSString stringWithFormat:@"%.1f%%",corrected*100/n]:@"—";}
static void Text(NSString *text,NSPoint point,NSColor *color,NSTextAlignment alignment){
    NSDictionary *attributes=@{NSFontAttributeName:[NSFont systemFontOfSize:11],NSForegroundColorAttributeName:color};
    NSSize size=[text sizeWithAttributes:attributes];if(alignment==NSTextAlignmentRight)point.x-=size.width;else if(alignment==NSTextAlignmentCenter)point.x-=size.width/2;
    [text drawAtPoint:point withAttributes:attributes];
}
@interface RTInputChart ()
@property(nonatomic,strong) NSTrackingArea *tracking;
@property(nonatomic) NSInteger hovered;
@end
@implementation RTInputChart
- (instancetype)initWithFrame:(NSRect)frame{
    if((self=[super initWithFrame:frame])){_selectedIndex=-1;_hovered=-1;_unit=@"字";_rows=@[];self.accessibilityElement=YES;self.accessibilityRole=NSAccessibilityGroupRole;}
    return self;
}
- (BOOL)isFlipped{return YES;}
- (BOOL)acceptsFirstResponder{return YES;}
- (NSRect)plot{return NSMakeRect(45,30,MAX(1,NSWidth(self.bounds)-88),MAX(1,NSHeight(self.bounds)-60));}
- (void)setRows:(NSArray<NSDictionary *> *)rows{_rows=[rows copy];[self removeAllToolTips];NSRect p=self.plot;for(NSUInteger i=0;i<MIN(6,rows.count);i++)[self addToolTipRect:NSMakeRect(NSMinX(p)+NSWidth(p)*i/6,NSMinY(p),NSWidth(p)/6,NSHeight(p)) owner:self userData:(void *)(uintptr_t)i];self.accessibilityLabel=self.summary;self.needsDisplay=YES;}
- (void)setSelectedIndex:(NSInteger)index{_selectedIndex=index;self.needsDisplay=YES;}
- (NSString *)summary{
    double n=0,c=0,u=0;for(NSDictionary *row in self.rows){n+=[row[@"inputs"] doubleValue];c+=[row[@"corrected"] doubleValue];u+=[row[@"units"] doubleValue];}
    return [NSString stringWithFormat:@"成功输入 %.0f 次 · 回改 %.0f 次\n总体回改率 %@\n平均上屏长度 %@\n上屏前修改拼写，不代表输错率。",n,c,Rate(n,c),n>0?[NSString stringWithFormat:@"%.1f %@/次",u/n,self.unit]:@"—"];
}
- (NSString *)tooltipForIndex:(NSUInteger)i{
    if(i>=self.rows.count||i>=6)return @"";NSDictionary *row=self.rows[i];double n=[row[@"inputs"] doubleValue],c=[row[@"corrected"] doubleValue],total=0;
    for(NSDictionary *r in self.rows)total+=[r[@"inputs"] doubleValue];
    return [NSString stringWithFormat:@"%@ %@/次\n输入 %.0f 次 · 占比 %.1f%%\n回改 %.0f 次 · 回改率 %@\n平均上屏长度 %@%@",Labels()[i],self.unit,n,total>0?n*100/total:0,c,Rate(n,c),n>0?[NSString stringWithFormat:@"%.1f %@",[row[@"units"] doubleValue]/n,self.unit]:@"—",n>0&&n<5?@"\n样本较少":@""];
}
- (NSString *)view:(NSView *)view stringForToolTip:(NSToolTipTag)tag point:(NSPoint)point userData:(void *)data{(void)view;(void)tag;(void)point;return [self tooltipForIndex:(NSUInteger)(uintptr_t)data];}
- (void)updateTrackingAreas{[super updateTrackingAreas];if(self.tracking)[self removeTrackingArea:self.tracking];self.tracking=[[NSTrackingArea alloc] initWithRect:NSZeroRect options:NSTrackingMouseMoved|NSTrackingMouseEnteredAndExited|NSTrackingActiveInKeyWindow|NSTrackingInVisibleRect owner:self userInfo:nil];[self addTrackingArea:self.tracking];}
- (NSInteger)indexAt:(NSPoint)point{NSRect p=self.plot;return NSPointInRect(point,p)?MIN(5,(NSInteger)((point.x-NSMinX(p))*6/NSWidth(p))):-1;}
- (void)mouseMoved:(NSEvent *)event{NSInteger index=[self indexAt:[self convertPoint:event.locationInWindow fromView:nil]];if(index!=self.hovered){self.hovered=index;self.needsDisplay=YES;}}
- (void)mouseExited:(NSEvent *)event{(void)event;self.hovered=-1;self.needsDisplay=YES;}
- (void)mouseDown:(NSEvent *)event{NSInteger index=[self indexAt:[self convertPoint:event.locationInWindow fromView:nil]];self.selectedIndex=index==self.selectedIndex?-1:index;if(self.selectionChanged)self.selectionChanged(self.selectedIndex);[self.window makeFirstResponder:self];}
- (void)keyDown:(NSEvent *)event{if(event.keyCode==53)self.selectedIndex=-1;else if(event.keyCode==123)self.selectedIndex=MAX(0,self.selectedIndex-1);else if(event.keyCode==124)self.selectedIndex=MIN(5,self.selectedIndex+1);else{[super keyDown:event];return;}if(self.selectionChanged)self.selectionChanged(self.selectedIndex);}
- (void)drawRect:(NSRect)dirty{
    (void)dirty;[[NSColor colorWithCalibratedWhite:1 alpha:1] setFill];NSRectFill(self.bounds);NSRect p=self.plot;double total=0;for(NSDictionary *row in self.rows)total+=[row[@"inputs"] doubleValue];
    Text(@"输入占比",NSMakePoint(0,0),Bar(),NSTextAlignmentLeft);Text(@"回改率",NSMakePoint(NSWidth(self.bounds),0),Line(),NSTextAlignmentRight);
    for(NSUInteger tick=0;tick<=4;tick++){double f=tick/4.;CGFloat y=NSMaxY(p)-NSHeight(p)*f;[NSColor.separatorColor setStroke];NSBezierPath *line=[NSBezierPath bezierPath];[line moveToPoint:NSMakePoint(NSMinX(p),y)];[line lineToPoint:NSMakePoint(NSMaxX(p),y)];[line stroke];Text([NSString stringWithFormat:@"%.0f%%",f*100],NSMakePoint(NSMinX(p)-6,y-6),NSColor.secondaryLabelColor,NSTextAlignmentRight);Text([NSString stringWithFormat:@"%.0f%%",f*100],NSMakePoint(NSMaxX(p)+6,y-6),Line(),NSTextAlignmentLeft);}
    CGFloat step=NSWidth(p)/6;NSPoint previous=NSZeroPoint;BOOL hasPrevious=NO;
    for(NSUInteger i=0;i<MIN(6,self.rows.count);i++){NSDictionary *row=self.rows[i];double n=[row[@"inputs"] doubleValue];CGFloat x=NSMinX(p)+step*(i+.5);BOOL selected=(NSInteger)i==(self.hovered>=0?self.hovered:self.selectedIndex);
        if(selected){[[Bar() colorWithAlphaComponent:.06] setFill];NSRectFillUsingOperation(NSMakeRect(x-step/2,NSMinY(p),step,NSHeight(p)),NSCompositingOperationSourceOver);}
        if(n>0){CGFloat h=NSHeight(p)*(total>0?n/total:0);[[Bar() colorWithAlphaComponent:selected?1:.7] setFill];[[NSBezierPath bezierPathWithRoundedRect:NSMakeRect(x-step*.24,NSMaxY(p)-h,step*.48,h) xRadius:3 yRadius:3] fill];NSPoint point=NSMakePoint(x,NSMaxY(p)-NSHeight(p)*[row[@"corrected"] doubleValue]/n);[Line() setStroke];if(hasPrevious){NSBezierPath *line=[NSBezierPath bezierPath];line.lineWidth=2;[line moveToPoint:previous];[line lineToPoint:point];[line stroke];}[Line() setFill];CGFloat radius=selected?4.5:3;[[NSBezierPath bezierPathWithOvalInRect:NSMakeRect(point.x-radius,point.y-radius,radius*2,radius*2)] fill];previous=point;hasPrevious=YES;}else hasPrevious=NO;
        Text(Labels()[i],NSMakePoint(x,NSMaxY(p)+10),NSColor.secondaryLabelColor,NSTextAlignmentCenter);
    }
    if(total==0)Text(@"暂无新口径样本",NSMakePoint(NSMidX(p),NSMidY(p)),NSColor.secondaryLabelColor,NSTextAlignmentCenter);
    if(self.selectedIndex>=0&&self.selectedIndex<(NSInteger)self.rows.count){CGFloat width=MIN(258,NSWidth(p)),x=MIN(NSMaxX(p)-width,MAX(NSMinX(p),NSMinX(p)+step*(self.selectedIndex+.5)-width/2));NSRect box=NSMakeRect(x,NSMinY(p)+8,width,86);[[NSColor.windowBackgroundColor colorWithAlphaComponent:.98] setFill];NSBezierPath *path=[NSBezierPath bezierPathWithRoundedRect:box xRadius:6 yRadius:6];[path fill];[NSColor.separatorColor setStroke];[path stroke];[[self tooltipForIndex:self.selectedIndex] drawInRect:NSInsetRect(box,9,6) withAttributes:@{NSFontAttributeName:[NSFont systemFontOfSize:11],NSForegroundColorAttributeName:NSColor.labelColor}];}
}
@end
